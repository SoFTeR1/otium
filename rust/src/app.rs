//! The shell: splash, then the home screen with the game row, background art and Play button.

use crate::data::{image_uri, Game, Library};
use crate::draw::{self, black, bold, font, semibold, white};
use crate::input::{Input, Nav, PadKind};
use egui::{pos2, vec2, Align2, Color32, CornerRadius, Rect, Stroke, StrokeKind};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Zone {
    Row,
    Play,
    More,
}

struct Toast {
    text: String,
    error: bool,
    at: Instant,
}

pub struct App {
    lib: Library,
    games: Vec<Game>,
    input: Input,
    sel: usize,
    zone: Zone,
    scroll: f32,
    /// background of the previous game, faded out under the new one
    bg_prev: Option<String>,
    bg_changed: Instant,
    splash_start: Option<Instant>,
    toast: Option<Toast>,
    fullscreen: bool,
    fs_tries: u32,
    started: Instant,
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        egui_extras::install_image_loaders(&cc.egui_ctx);
        crate::fonts::install(&cc.egui_ctx);
        let mut style = (*cc.egui_ctx.style()).clone();
        style.visuals = egui::Visuals::dark();
        style.visuals.panel_fill = Color32::from_rgb(6, 8, 14);
        cc.egui_ctx.set_style(style);

        let lib = Library::load();
        let games = lib.sorted_games();
        let splash = lib.setting_bool("splash", true);
        let fullscreen = lib.setting_bool("fullscreen", true);
        Self {
            games,
            input: Input::new(),
            sel: 0,
            zone: Zone::Row,
            scroll: 0.0,
            bg_prev: None,
            bg_changed: Instant::now() - Duration::from_secs(5),
            splash_start: splash.then(Instant::now),
            toast: None,
            fullscreen,
            fs_tries: 0,
            started: Instant::now(),
            lib,
        }
    }

    fn select(&mut self, i: usize) {
        if i == self.sel || i >= self.games.len() {
            return;
        }
        self.bg_prev = self.games.get(self.sel).and_then(background_of);
        self.bg_changed = Instant::now();
        self.sel = i;
    }

    fn handle(&mut self, nav: Nav) {
        let n = self.games.len();
        match (self.zone, nav) {
            (Zone::Row, Nav::Left) if self.sel > 0 => self.select(self.sel - 1),
            (Zone::Row, Nav::Right) if self.sel + 1 < n => self.select(self.sel + 1),
            (Zone::Row, Nav::Down) if n > 0 => self.zone = Zone::Play,
            (Zone::Row, Nav::Ok) => self.launch_selected(),
            (Zone::Play, Nav::Ok) => self.launch_selected(),
            (Zone::Play, Nav::Right) => self.zone = Zone::More,
            (Zone::More, Nav::Left) => self.zone = Zone::Play,
            (Zone::Play | Zone::More, Nav::Up | Nav::Back) => self.zone = Zone::Row,
            (Zone::More, Nav::Ok) | (_, Nav::Options) => {
                self.toast("Параметры игры появятся в следующей сборке", false)
            }
            (_, Nav::Add) => self.toast("Добавление игр появится в следующей сборке", false),
            _ => {}
        }
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
        match crate::launch::launch(&game) {
            Ok(()) => self.toast(&format!("Запуск: {}", game.name), false),
            Err(e) => self.toast(&e, true),
        }
    }

    fn toast(&mut self, text: &str, error: bool) {
        self.toast = Some(Toast { text: text.into(), error, at: Instant::now() });
    }
}

fn background_of(g: &Game) -> Option<String> {
    g.hero.as_deref().or(g.cover.as_deref()).and_then(image_uri)
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        ctx.request_repaint_after(Duration::from_millis(16));
        let navs = self.input.poll(ctx);

        if ctx.input(|i| i.key_pressed(egui::Key::F11)) {
            self.fullscreen = !self.fullscreen;
            self.fs_tries = 0;
            ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.fullscreen));
        }
        self.keep_fullscreen(ctx);

        if let Some(start) = self.splash_start {
            let skip = !navs.is_empty() || ctx.input(|i| i.pointer.any_click());
            if skip || start.elapsed() > SPLASH {
                self.splash_start = None;
                self.started = Instant::now();
            }
        } else {
            for nav in navs {
                self.handle(nav);
            }
        }

        egui::CentralPanel::default().frame(egui::Frame::NONE.fill(Color32::from_rgb(6, 8, 14))).show(ctx, |ui| {
            match self.splash_start {
                Some(start) => splash(ui, start.elapsed().as_secs_f32()),
                None => self.home(ui),
            }
        });
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

        self.paint_background(ui, rect);
        self.paint_top_bar(ui, rect, s, appear);

        if self.games.is_empty() {
            let p = ui.painter();
            p.text(rect.center() - vec2(0.0, 20.0 * s), Align2::CENTER_CENTER, "Библиотека пуста", semibold(40.0 * s), white(appear));
            p.text(
                rect.center() + vec2(0.0, 30.0 * s),
                Align2::CENTER_CENTER,
                format!("OTIUM читает игры из {}", Library::file().display()),
                font(20.0 * s),
                white(0.6 * appear),
            );
            return;
        }

        // ---- game row: the selected tile stays at the left edge, the rest scroll past it ----
        let tile = 116.0 * s;
        let big = 164.0 * s;
        let gap = 20.0 * s;
        let left = 96.0 * s;
        let row_y = 150.0 * s;
        let k = 1.0 - (-dt * 16.0).exp();
        self.scroll += (self.sel as f32 - self.scroll) * k;

        let in_row = self.zone == Zone::Row;
        for (i, g) in self.games.iter().enumerate() {
            let d = i as f32 - self.scroll;
            // the selected slot is wider, so tiles after it shift right by the difference
            let shift = (d.clamp(0.0, 1.0)) * (big - tile);
            let x = left + d * (tile + gap) + shift;
            if x > rect.right() || x + big < rect.left() - big {
                continue;
            }
            let focus = 1.0 - (i as f32 - self.scroll).abs().min(1.0);
            let size = tile + (big - tile) * focus;
            let r = Rect::from_min_size(pos2(x, row_y), vec2(size, size));
            let fade = if d < -0.5 { (1.0 + d).max(0.0) } else { 1.0 } * appear;
            paint_tile(ui, g, r, 14.0 * s, fade, i == self.sel && in_row, s);
        }

        // name of the selected game sits to the right of its tile, PS5-style
        let g = &self.games[self.sel];
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
        let info_y = rect.height() * 0.50;
        let logo_box = Rect::from_min_size(pos2(left, info_y - 190.0 * s), vec2(560.0 * s, 180.0 * s));
        let logo = g.logo.as_deref().and_then(image_uri);
        let painted = logo.and_then(|u| draw::contain_left(ui, &u, logo_box, white(appear)));
        if painted.is_none() {
            ui.painter().text(logo_box.left_bottom(), Align2::LEFT_BOTTOM, &g.name, bold(64.0 * s), white(appear));
        }

        let btn = Rect::from_min_size(pos2(left, info_y + 20.0 * s), vec2(280.0 * s, 68.0 * s));
        let play_focus = self.zone == Zone::Play;
        let p = ui.painter();
        if play_focus {
            draw::glow(p, btn, 34.0 * s, Color32::WHITE, 18.0 * s, 0.6);
        }
        p.rect_filled(btn, CornerRadius::same((34.0 * s) as u8), if play_focus { Color32::WHITE } else { white(0.88) });
        p.text(btn.center(), Align2::CENTER_CENTER, "Играть", semibold(26.0 * s), Color32::from_rgb(10, 12, 18));

        let more = Rect::from_center_size(pos2(btn.right() + 24.0 * s + 34.0 * s, btn.center().y), vec2(68.0 * s, 68.0 * s));
        let more_focus = self.zone == Zone::More;
        if more_focus {
            draw::glow(p, more, 34.0 * s, Color32::WHITE, 18.0 * s, 0.6);
        }
        p.circle_filled(more.center(), 34.0 * s, if more_focus { white(0.95) } else { black(0.45) });
        p.circle_stroke(more.center(), 34.0 * s, Stroke::new(1.5_f32, white(0.5)));
        for dx in [-12.0, 0.0, 12.0] {
            p.circle_filled(more.center() + vec2(dx * s, 0.0), 3.5 * s, if more_focus { Color32::BLACK } else { Color32::WHITE });
        }

        let mut info = Vec::new();
        if let Some(pt) = g.playtime.filter(|m| *m > 0.0) {
            info.push(format_playtime(pt));
        }
        if g.steam_app_id().is_some() {
            info.push("Steam".to_string());
        } else if g.category() == "app" {
            info.push("Приложение".to_string());
        }
        if !info.is_empty() {
            p.text(pos2(left + 4.0 * s, btn.bottom() + 26.0 * s), Align2::LEFT_TOP, info.join("  ·  "), font(20.0 * s), white(0.7 * appear));
        }

        self.paint_hints(ui, rect, s);
        self.paint_toast(ui, rect, s);
    }

    fn paint_background(&self, ui: &egui::Ui, rect: Rect) {
        let p = ui.painter();
        let cur = self.games.get(self.sel).and_then(background_of);
        let t = (self.bg_changed.elapsed().as_secs_f32() / 0.45).min(1.0);
        let dim = Color32::from_gray(150);
        if t < 1.0 {
            if let Some(prev) = &self.bg_prev {
                draw::cover(ui, prev, rect, 0.0, dim, (0.5, 0.3));
            }
        }
        if let Some(cur) = &cur {
            let a = (ease_out(t) * 150.0) as u8;
            draw::cover(ui, cur, rect, 0.0, Color32::from_rgba_unmultiplied(a, a, a, (ease_out(t) * 255.0) as u8), (0.5, 0.3));
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
        let x = 96.0 * s;
        let games = p.text(pos2(x, y), Align2::LEFT_CENTER, "Игры", semibold(30.0 * s), white(a));
        p.text(pos2(games.right() + 36.0 * s, y), Align2::LEFT_CENTER, "Медиа", semibold(30.0 * s), white(0.5 * a));

        let right = rect.right() - 96.0 * s;
        let clock = chrono::Local::now().format("%H:%M").to_string();
        let c = p.text(pos2(right, y), Align2::RIGHT_CENTER, clock, font(28.0 * s), white(a));

        let user = self.lib.setting_str("user");
        let user = if user.is_empty() { "Игрок" } else { user };
        let av = pos2(c.left() - 40.0 * s, y);
        p.circle_filled(av, 22.0 * s, draw::name_color(user));
        p.text(av, Align2::CENTER_CENTER, draw::initials(user), semibold(17.0 * s), white(a));
        if self.input.pad_connected {
            p.text(pos2(av.x - 40.0 * s, y), Align2::RIGHT_CENTER, "🎮", font(24.0 * s), white(0.75 * a));
        }
    }

    fn paint_hints(&self, ui: &egui::Ui, rect: Rect, s: f32) {
        let ps = self.input.pad_kind == PadKind::PlayStation;
        let hints: [(&str, &str); 3] = if ps {
            [("✕", "Выбрать"), ("○", "Назад"), ("△", "Параметры")]
        } else {
            [("A", "Выбрать"), ("B", "Назад"), ("Y", "Параметры")]
        };
        let p = ui.painter();
        let mut x = rect.right() - 96.0 * s;
        let y = rect.bottom() - 54.0 * s;
        for (glyph, label) in hints.iter().rev() {
            let t = p.text(pos2(x, y), Align2::RIGHT_CENTER, *label, font(19.0 * s), white(0.75));
            let c = pos2(t.left() - 22.0 * s, y);
            draw::pad_glyph(p, c, 14.0 * s, glyph, Color32::WHITE);
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

fn paint_tile(ui: &egui::Ui, g: &Game, r: Rect, radius: f32, alpha: f32, focused: bool, s: f32) {
    if alpha <= 0.01 {
        return;
    }
    let p = ui.painter();
    if focused {
        draw::glow(p, r, radius, Color32::WHITE, 16.0 * s, 0.55 * alpha);
    }
    let tint = white(alpha);
    let art = g.cover.as_deref().or(g.hero.as_deref()).and_then(image_uri);
    let ok = art.is_some_and(|u| draw::cover(ui, &u, r, radius, tint, (0.5, 0.35)));
    if !ok {
        let cr = CornerRadius::same(radius as u8);
        p.rect_filled(r, cr, draw::name_color(&g.name).gamma_multiply(alpha));
        p.text(r.center(), Align2::CENTER_CENTER, draw::initials(&g.name), bold(r.height() * 0.32), white(0.9 * alpha));
    }
    if focused {
        p.rect_stroke(r, CornerRadius::same(radius as u8), Stroke::new(3.0 * s, white(alpha)), StrokeKind::Outside);
    }
}

fn format_playtime(minutes: f64) -> String {
    let m = minutes.round() as i64;
    if m < 60 {
        format!("{m} мин")
    } else {
        format!("{} ч {} мин", m / 60, m % 60)
    }
}

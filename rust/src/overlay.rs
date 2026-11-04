//! The overlay: a transparent, always-on-top window over the game, like `overlay.html` in the
//! Electron build. It shows the voice widget (who's in the call, who's talking), the incoming call
//! card and notifications while OTIUM itself isn't in front, without taking focus from the game.
//!
//! It is an egui viewport of the main window, so it shares fonts and the event loop. The window
//! exists only while it has something on it, is click-through everywhere except its buttons, and
//! never activates (WS_EX_NOACTIVATE): clicking "Принять" leaves the game in front.

use crate::draw::{self, bold, font, semibold, white};
use egui::{pos2, vec2, Align2, Color32, CornerRadius, Pos2, Rect, Stroke, StrokeKind, ViewportId};
use std::time::{Duration, Instant};

pub const TITLE: &str = "OTIUM overlay";
const TOAST_LIFE: Duration = Duration::from_secs(6);
const MAX_TOASTS: usize = 3;
const GREEN: Color32 = Color32::from_rgb(35, 165, 90);
const RED: Color32 = Color32::from_rgb(239, 68, 68);

pub struct Toast {
    pub title: String,
    pub text: String,
    /// (key for the colour, name for the letter) — a friend or a group
    pub avatar: Option<(String, String)>,
    /// a picture instead (a trophy's icon)
    pub img: Option<String>,
    pub error: bool,
    at: Instant,
}

pub struct Person {
    pub key: String,
    pub name: String,
    pub muted: bool,
    pub deafened: bool,
    pub speaking: bool,
    pub connected: bool,
}

pub enum CallView {
    Incoming { key: String, name: String, kicker: String, group: bool },
    Active { title: String, status: Option<String>, started: Option<Instant>, people: Vec<Person> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Accept,
    Decline,
    OpenCall,
}

#[derive(Default)]
pub struct Overlay {
    toasts: Vec<Toast>,
    hwnd: isize,
    /// clickable areas from the last frame, in overlay points
    regions: Vec<(Rect, Option<Action>)>,
    clickable: bool,
    /// when the current call view appeared (for the slide-in), and which kind it was
    call_since: Option<(bool, Instant)>,
    restack_at: Option<Instant>,
    last_ex: isize,
}

impl Overlay {
    pub fn toast(&mut self, title: &str, text: &str, avatar: Option<(String, String)>, error: bool) {
        self.toasts.push(Toast { title: title.into(), text: text.into(), avatar, img: None, error, at: Instant::now() });
        let extra = self.toasts.len().saturating_sub(MAX_TOASTS);
        self.toasts.drain(..extra);
    }

    pub fn toast_img(&mut self, title: &str, text: &str, img: Option<String>) {
        self.toast(title, text, None, false);
        if let Some(t) = self.toasts.last_mut() {
            t.img = img;
        }
    }

    /// Draws the overlay window for this frame, or closes it when there is nothing to show.
    /// `own` is the main window (the overlay covers its monitor). Returns a clicked button.
    pub fn show(&mut self, ctx: &egui::Context, own: isize, call: Option<CallView>) -> Option<Action> {
        self.toasts.retain(|t| t.at.elapsed() < TOAST_LIFE);
        let kind = call.as_ref().map(|c| matches!(c, CallView::Incoming { .. }));
        match (kind, self.call_since) {
            (Some(k), Some((was, _))) if k == was => {}
            (Some(k), _) => self.call_since = Some((k, Instant::now())),
            (None, _) => self.call_since = None,
        }
        if call.is_none() && self.toasts.is_empty() {
            // not calling show_viewport_* closes the window: nothing sits over the game for nothing
            self.hwnd = 0;
            self.clickable = false;
            self.regions.clear();
            return None;
        }
        let Some(mon) = crate::win::monitor_rect(own) else { return None };
        let ppp = ctx.input(|i| i.viewport().native_pixels_per_point).unwrap_or(1.0);

        // click-through except over the buttons: Windows sends a click-through window no mouse
        // moves, so the cursor is checked against last frame's buttons instead
        let over = crate::win::cursor_pos().and_then(|(x, y)| {
            let p = pos2((x - mon.0) as f32 / ppp, (y - mon.1) as f32 / ppp);
            self.regions.iter().find(|(r, _)| r.contains(p)).map(|(_, a)| *a)
        });
        self.clickable = over.is_some();

        let builder = egui::ViewportBuilder::default()
            .with_title(TITLE)
            .with_decorations(false)
            .with_transparent(true)
            .with_always_on_top()
            .with_taskbar(false)
            .with_active(false)
            .with_resizable(false)
            .with_mouse_passthrough(!self.clickable)
            .with_position(pos2(mon.0 as f32 / ppp, mon.1 as f32 / ppp))
            .with_inner_size(vec2(mon.2 as f32 / ppp, mon.3 as f32 / ppp));

        let id = ViewportId::from_hash_of("otium-overlay");
        let toasts = &self.toasts;
        let call_since = self.call_since.map(|(_, t)| t);
        let (regions, clicked) = ctx.show_viewport_immediate(id, builder, |ctx, _| {
            let mut regions = Vec::new();
            let mut clicked = None;
            egui::CentralPanel::default().frame(egui::Frame::NONE).show(ctx, |ui| {
                let rect = ui.max_rect();
                let s = rect.height() / 1080.0;
                let hover = ui.input(|i| i.pointer.hover_pos());
                let click = ui.input(|i| if i.pointer.primary_clicked() { i.pointer.interact_pos() } else { None });
                match &call {
                    Some(CallView::Incoming { key, name, kicker, group }) => {
                        incoming(ui, rect, s, key, name, kicker, *group, call_since, hover, &mut regions);
                    }
                    Some(CallView::Active { title, status, started, people }) => {
                        voice_widget(ui, rect, s, title, status.as_deref(), *started, people, hover, &mut regions);
                    }
                    None => {}
                }
                paint_toasts(ui, rect, s, toasts);
                if let Some(c) = click {
                    clicked = regions.iter().find(|(r, _): &&(Rect, Option<Action>)| r.contains(c)).and_then(|(_, a)| *a);
                }
            });
            (regions, clicked)
        });
        self.regions = regions;

        if self.hwnd == 0 || !crate::win::is_window(self.hwnd) {
            self.hwnd = crate::win::find_own_window(TITLE);
            self.last_ex = 0;
        }
        if self.hwnd != 0 {
            // games that make themselves topmost can climb over us: restack now and then
            let restack = self.restack_at.is_none_or(|t| t.elapsed() > Duration::from_secs(1));
            if restack {
                self.restack_at = Some(Instant::now());
            }
            crate::win::pin_overlay(self.hwnd, mon, restack, &mut self.last_ex);
        }
        // the clock and the speaking rings move: keep drawing while it's up
        ctx.request_repaint_after(Duration::from_millis(33)); // 30 fps: smooth slides, light on the game
        clicked
    }
}

fn slide(since: Option<Instant>) -> f32 {
    let t = since.map_or(1.0, |t| (t.elapsed().as_secs_f32() / 0.4).min(1.0));
    1.0 - (1.0 - t).powi(3)
}

fn letter(name: &str) -> String {
    name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_else(|| "?".into())
}

fn mmss(d: Duration) -> String {
    let s = d.as_secs();
    format!("{:02}:{:02}", s / 60, s % 60)
}

/// Windows-style call toast at the top right with Accept / Decline.
#[allow(clippy::too_many_arguments)]
fn incoming(ui: &egui::Ui, rect: Rect, s: f32, key: &str, name: &str, kicker: &str, group: bool, since: Option<Instant>, hover: Option<Pos2>, regions: &mut Vec<(Rect, Option<Action>)>) {
    let p = ui.painter();
    let k = slide(since);
    let w = 420.0 * s;
    let card = Rect::from_min_size(pos2(rect.right() - 24.0 * s - w + (1.0 - k) * (w + 40.0 * s), rect.top() + 24.0 * s), vec2(w, 196.0 * s));
    let r = CornerRadius::same((18.0 * s) as u8);
    draw::glow(p, card, 18.0 * s, Color32::BLACK, 30.0 * s, 0.5);
    p.rect_filled(card, r, Color32::from_rgba_unmultiplied(24, 30, 52, 246));
    p.rect_stroke(card, r, Stroke::new(1.0_f32, white(0.1)), StrokeKind::Inside);
    let av = pos2(card.left() + 52.0 * s, card.top() + 56.0 * s);
    let t = ui.input(|i| i.time) as f32;
    let phase = (t / 1.6) % 1.0;
    p.circle_stroke(av, 32.0 * s * (0.9 + 0.55 * phase), Stroke::new(2.0 * s, Color32::from_rgb(91, 227, 138).gamma_multiply(1.0 - phase)));
    p.circle_filled(av, 30.0 * s, draw::name_color(key));
    p.text(av, Align2::CENTER_CENTER, letter(name), bold(26.0 * s), Color32::WHITE);
    p.text(pos2(av.x + 46.0 * s, av.y - 13.0 * s), Align2::LEFT_CENTER, kicker.to_uppercase(), semibold(13.0 * s), Color32::from_rgb(159, 176, 216));
    p.text(pos2(av.x + 46.0 * s, av.y + 12.0 * s), Align2::LEFT_CENTER, name, semibold(23.0 * s), Color32::WHITE);

    let bw = (w - 52.0 * s) / 2.0;
    let by = card.top() + 104.0 * s;
    let buttons = [(if group { "Войти" } else { "Принять" }, Color32::from_rgb(34, 197, 94), Action::Accept), (if group { "Не сейчас" } else { "Отклонить" }, RED, Action::Decline)];
    for (i, (label, color, action)) in buttons.into_iter().enumerate() {
        let b = Rect::from_min_size(pos2(card.left() + 18.0 * s + i as f32 * (bw + 16.0 * s), by), vec2(bw, 48.0 * s));
        let hot = hover.is_some_and(|h| b.contains(h));
        p.rect_filled(b, CornerRadius::same((14.0 * s) as u8), if hot { color.gamma_multiply(1.15) } else { color });
        p.text(b.center(), Align2::CENTER_CENTER, label, semibold(18.0 * s), Color32::WHITE);
        regions.push((b, Some(action)));
    }
    p.text(pos2(card.center().x, card.bottom() - 22.0 * s), Align2::CENTER_CENTER, "или кнопка Xbox на геймпаде", font(14.0 * s), Color32::from_rgb(129, 145, 184));
}

/// Discord-style voice widget at the middle of the left edge: who's in, who's talking, who's
/// muted. Hovering it wakes it up; clicking brings OTIUM forward to the call.
#[allow(clippy::too_many_arguments)]
fn voice_widget(ui: &egui::Ui, rect: Rect, s: f32, title: &str, status: Option<&str>, started: Option<Instant>, people: &[Person], hover: Option<Pos2>, regions: &mut Vec<(Rect, Option<Action>)>) {
    let p = ui.painter();
    let row_h = 36.0 * s;
    let head_h = 26.0 * s;
    let h = head_h + 8.0 * s + people.len() as f32 * row_h;
    let left = rect.left() + 18.0 * s;
    let top = rect.center().y - h / 2.0;
    let area = Rect::from_min_size(pos2(left - 10.0 * s, top - 10.0 * s), vec2(300.0 * s, h + 20.0 * s + 26.0 * s));
    let hot = hover.is_some_and(|h| area.contains(h));
    regions.push((area, Some(Action::OpenCall)));
    if hot {
        p.rect_filled(area, CornerRadius::same((14.0 * s) as u8), Color32::from_rgba_unmultiplied(17, 19, 24, 190));
        p.rect_stroke(area, CornerRadius::same((14.0 * s) as u8), Stroke::new(1.0_f32, white(0.08)), StrokeKind::Inside);
    }
    let a = if hot { 1.0 } else { 0.92 };

    // header chip: ● Голосовой звонок 03:12
    let head = match status {
        Some(st) => st.to_string(),
        None => format!("{}  {}", title, started.map(|t| mmss(t.elapsed())).unwrap_or_default()),
    };
    let g = p.layout_no_wrap(head, semibold(14.0 * s), GREEN.gamma_multiply(a));
    let chip = Rect::from_min_size(pos2(left, top), vec2(g.size().x + 30.0 * s, head_h));
    p.rect_filled(chip, CornerRadius::same((6.0 * s) as u8), Color32::from_black_alpha((115.0 * a) as u8));
    p.circle_filled(pos2(chip.left() + 11.0 * s, chip.center().y), 3.5 * s, GREEN.gamma_multiply(a));
    p.galley(pos2(chip.left() + 21.0 * s, chip.center().y - g.size().y / 2.0), g, Color32::WHITE);

    let mut y = chip.bottom() + 8.0 * s;
    for person in people {
        let c = pos2(left + 17.0 * s, y + row_h / 2.0);
        if person.speaking {
            p.circle_stroke(c, 17.0 * s, Stroke::new(2.5 * s, GREEN));
            p.circle_filled(c, 22.0 * s, GREEN.gamma_multiply(0.15));
        }
        let col = draw::name_color(&person.key).gamma_multiply(if person.connected { a } else { 0.45 });
        p.circle_filled(c, 15.0 * s, col);
        let name = if person.name.trim().is_empty() { "Вы" } else { person.name.as_str() };
        p.text(c, Align2::CENTER_CENTER, letter(name), bold(15.0 * s), Color32::WHITE);
        let label = if person.connected { name.to_string() } else { format!("{name} · соединяем…") };
        let color = if person.deafened || !person.connected { Color32::from_rgb(181, 186, 193) } else { Color32::from_rgb(242, 243, 245) };
        let g = p.layout_no_wrap(label, semibold(15.0 * s), color);
        let w = g.size().x.min(230.0 * s);
        let icons = person.connected as usize * (person.muted as usize + person.deafened as usize);
        let chip = Rect::from_min_size(pos2(c.x + 24.0 * s, c.y - 13.0 * s), vec2(w + 20.0 * s + icons as f32 * 22.0 * s, 26.0 * s));
        p.rect_filled(chip, CornerRadius::same((6.0 * s) as u8), Color32::from_black_alpha((140.0 * a) as u8));
        p.with_clip_rect(chip).galley(pos2(chip.left() + 10.0 * s, chip.center().y - g.size().y / 2.0), g, Color32::WHITE);
        let mut ix = chip.left() + 10.0 * s + w + 13.0 * s;
        if person.connected && person.deafened {
            headphones_off(p, pos2(ix, chip.center().y), 7.0 * s);
            ix += 22.0 * s;
        }
        if person.connected && person.muted {
            mic_off(p, pos2(ix, chip.center().y), 7.0 * s);
        }
        y += row_h;
    }
    if hot {
        p.text(pos2(left, y + 6.0 * s), Align2::LEFT_TOP, "Открыть звонок в OTIUM ›", semibold(14.0 * s), Color32::from_rgb(143, 194, 255));
    }
}

const MUTE_RED: Color32 = Color32::from_rgb(242, 63, 67);

/// Crossed-out microphone, `r` is half its height.
fn mic_off(p: &egui::Painter, c: Pos2, r: f32) {
    let st = Stroke::new(r * 0.26, MUTE_RED);
    let capsule = Rect::from_center_size(c - vec2(0.0, r * 0.3), vec2(r * 0.8, r * 1.2));
    p.rect_filled(capsule, CornerRadius::same((r * 0.4) as u8), MUTE_RED);
    // the cradle under the capsule and the stand
    let pts: Vec<Pos2> = (0..=10).map(|i| {
        let t = std::f32::consts::PI * i as f32 / 10.0;
        c + vec2(-t.cos() * r * 0.75, r * 0.05 + t.sin() * r * 0.55)
    }).collect();
    p.add(egui::Shape::line(pts, st));
    p.line_segment([c + vec2(0.0, r * 0.6), c + vec2(0.0, r)], st);
    p.line_segment([c + vec2(-r * 0.9, -r * 0.9), c + vec2(r * 0.9, r * 0.9)], Stroke::new(r * 0.3, MUTE_RED));
}

/// Crossed-out headphones, `r` is half their height.
fn headphones_off(p: &egui::Painter, c: Pos2, r: f32) {
    let st = Stroke::new(r * 0.26, MUTE_RED);
    let pts: Vec<Pos2> = (0..=12).map(|i| {
        let t = std::f32::consts::PI * i as f32 / 12.0;
        c + vec2(-t.cos() * r * 0.85, r * 0.1 - t.sin() * r * 0.85)
    }).collect();
    p.add(egui::Shape::line(pts, st));
    for side in [-1.0, 1.0] {
        p.rect_filled(Rect::from_center_size(c + vec2(side * r * 0.7, r * 0.45), vec2(r * 0.45, r * 0.8)), CornerRadius::same((r * 0.15) as u8), MUTE_RED);
    }
    p.line_segment([c + vec2(-r * 0.95, -r * 0.95), c + vec2(r * 0.95, r * 0.95)], Stroke::new(r * 0.3, MUTE_RED));
}

/// Windows 11-style notifications at the bottom right, newest at the bottom.
fn paint_toasts(ui: &egui::Ui, rect: Rect, s: f32, toasts: &[Toast]) {
    let p = ui.painter();
    let w = 400.0 * s;
    let mut bottom = rect.bottom() - 24.0 * s;
    for t in toasts.iter().rev() {
        let age = t.at.elapsed().as_secs_f32();
        let life = TOAST_LIFE.as_secs_f32();
        let k_in = 1.0 - (1.0 - (age / 0.42).min(1.0)).powi(3);
        let k_out = ((life - age) / 0.4).clamp(0.0, 1.0);
        let k = k_in * k_out;
        let text_w = w - 96.0 * s;
        let body = p.layout(t.text.clone(), font(15.0 * s), Color32::from_rgb(213, 219, 232).gamma_multiply(k), text_w);
        // at most three lines, like the Electron toast
        let line_h = body.rows.first().map_or(18.0 * s, |r| r.height());
        let body_h = body.size().y.min(line_h * 3.0);
        let h = (44.0 * s + 26.0 * s + body_h + 14.0 * s).max(112.0 * s);
        let card = Rect::from_min_size(pos2(rect.right() - 24.0 * s - w + (1.0 - k_in) * (w + 40.0 * s), bottom - h), vec2(w, h));
        let r = CornerRadius::same((12.0 * s) as u8);
        draw::glow(p, card, 12.0 * s, Color32::BLACK, 24.0 * s, 0.45 * k);
        p.rect_filled(card, r, Color32::from_rgba_unmultiplied(32, 34, 42, (240.0 * k) as u8));
        p.rect_stroke(card, r, Stroke::new(1.0_f32, white(0.08 * k)), StrokeKind::Inside);
        if t.error {
            p.rect_filled(Rect::from_min_size(card.min, vec2(3.0 * s, h)), CornerRadius { nw: (12.0 * s) as u8, sw: (12.0 * s) as u8, ne: 0, se: 0 }, Color32::from_rgb(255, 77, 99).gamma_multiply(k));
        }
        // app line: a small OTIUM badge
        let badge = Rect::from_min_size(pos2(card.left() + 14.0 * s, card.top() + 13.0 * s), vec2(16.0 * s, 16.0 * s));
        p.rect_filled(badge, CornerRadius::same((4.0 * s) as u8), Color32::from_rgb(61, 123, 255).gamma_multiply(k));
        p.text(badge.center(), Align2::CENTER_CENTER, "O", bold(11.0 * s), white(k));
        p.text(pos2(badge.right() + 8.0 * s, badge.center().y), Align2::LEFT_CENTER, "OTIUM", font(13.0 * s), Color32::from_rgb(185, 192, 208).gamma_multiply(k));
        // picture: friend's letter or a gamepad
        let pic = Rect::from_min_size(pos2(card.left() + 14.0 * s, card.top() + 44.0 * s), vec2(50.0 * s, 50.0 * s));
        let pictured = t.img.as_ref().is_some_and(|u| draw::cover(ui, &[u.clone()], pic, 10.0 * s, white(k), (0.5, 0.5)));
        match &t.avatar {
            _ if pictured => {}
            Some((key, name)) => {
                p.circle_filled(pic.center(), 25.0 * s, draw::name_color(key).gamma_multiply(k));
                p.text(pic.center(), Align2::CENTER_CENTER, letter(name), bold(21.0 * s), white(k));
            }
            None => {
                p.rect_filled(pic, CornerRadius::same((10.0 * s) as u8), white(0.08 * k));
                p.text(pic.center(), Align2::CENTER_CENTER, "🎮", font(26.0 * s), white(k));
            }
        }
        let tx = pic.right() + 14.0 * s;
        p.text(pos2(tx, pic.top() + 2.0 * s), Align2::LEFT_TOP, &t.title, semibold(16.0 * s), white(k));
        let body_rect = Rect::from_min_size(pos2(tx, pic.top() + 26.0 * s), vec2(text_w, body_h));
        p.with_clip_rect(body_rect).galley(body_rect.min, body, Color32::WHITE);
        bottom = card.top() - 10.0 * s;
    }
}

//! Settings → "Геймпад: проверка и калибровка", like the Electron build's pad test: every button
//! lights up, sticks move inside their rings, triggers fill up. Calibration measures each stick's
//! resting centre and jitter (→ dead zone), then its reach while you roll it around (→ range).
//! Inside the test every button is "being tested", so leaving and calibrating are holds: hold B/○
//! to leave, hold Start to calibrate, hold Y/△ to reset. The keyboard works as usual.

use crate::draw::{font, semibold, white};
use crate::input::{AxisCal, PadCal, PadState};
use egui::{pos2, vec2, Align2, Color32, CornerRadius, Rect, Stroke, StrokeKind};
use std::time::{Duration, Instant};

const HOLD: Duration = Duration::from_millis(1200);
const REST: Duration = Duration::from_millis(2500);
const CIRCLE: Duration = Duration::from_millis(5000);
const BLUE: Color32 = Color32::from_rgb(61, 123, 255);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Phase {
    Idle,
    Rest,
    Circle,
}

#[derive(Default, Clone)]
struct Samples {
    sum: f64,
    n: u32,
    vals: Vec<f32>,
    lo: f32,
    hi: f32,
}

pub enum Out {
    Close,
    Save(String, PadCal),
    Reset(String),
}

pub struct PadTest {
    phase: Phase,
    t0: Instant,
    key: String,
    samples: Vec<Samples>,
    /// when B, Start, Y started being held; None = up, Some(None) = already fired this hold
    holds: [Option<Option<Instant>>; 3],
    state: Option<PadState>,
    cal: Option<PadCal>,
    /// asked from the keyboard
    want_start: bool,
    want_reset: bool,
}

impl PadTest {
    pub fn new() -> Self {
        Self {
            phase: Phase::Idle,
            t0: Instant::now(),
            key: String::new(),
            samples: Vec::new(),
            holds: [None; 3],
            state: None,
            cal: None,
            want_start: false,
            want_reset: false,
        }
    }

    pub fn start(&mut self) {
        self.want_start = true;
    }

    pub fn reset(&mut self) {
        self.want_reset = true;
    }

    fn begin(&mut self) {
        let Some(st) = &self.state else { return };
        self.key = st.key.clone();
        self.phase = Phase::Rest;
        self.t0 = Instant::now();
        self.samples = vec![Samples { lo: 1.0, hi: -1.0, ..Default::default() }; 4];
    }

    /// Called every frame with the pad in hand and its current calibration.
    pub fn tick(&mut self, st: Option<PadState>, cal: Option<PadCal>) -> Option<Out> {
        self.state = st;
        self.cal = cal;
        let now = Instant::now();
        let mut out = None;
        if let Some(st) = self.state.clone() {
            for (i, down) in [st.back, st.start, st.north].into_iter().enumerate() {
                match (down, self.holds[i]) {
                    (false, _) => self.holds[i] = None,
                    (true, None) => self.holds[i] = Some(Some(now)),
                    (true, Some(Some(t))) if now - t >= HOLD => {
                        self.holds[i] = Some(None); // fires once per hold
                        match i {
                            0 => return Some(Out::Close),
                            1 if self.phase == Phase::Idle => self.want_start = true,
                            2 if self.phase == Phase::Idle => self.want_reset = true,
                            _ => {}
                        }
                    }
                    _ => {}
                }
            }
        }
        if std::mem::take(&mut self.want_reset) {
            if let Some(st) = &self.state {
                if self.cal.is_some() {
                    out = Some(Out::Reset(st.key.clone()));
                }
            }
        }
        if std::mem::take(&mut self.want_start) && self.phase == Phase::Idle {
            self.begin();
        }
        let el = now - self.t0;
        match (self.phase, &self.state) {
            (Phase::Rest, Some(st)) => {
                for (s, v) in self.samples.iter_mut().zip(st.raw) {
                    s.sum += v as f64;
                    s.n += 1;
                    s.vals.push(v);
                }
                if el > REST {
                    self.phase = Phase::Circle;
                    self.t0 = now;
                    crate::sound::play(crate::sound::Sfx::Move);
                }
            }
            (Phase::Circle, Some(st)) => {
                for (s, v) in self.samples.iter_mut().zip(st.raw) {
                    s.lo = s.lo.min(v);
                    s.hi = s.hi.max(v);
                }
                if el > CIRCLE {
                    self.phase = Phase::Idle;
                    return Some(Out::Save(self.key.clone(), result(&self.samples)));
                }
            }
            _ => {}
        }
        out
    }

    pub fn paint(&self, ui: &egui::Ui, screen: Rect, s: f32, a: f32) {
        let p = ui.painter();
        p.rect_filled(screen, 0.0, Color32::from_black_alpha((0.62 * 255.0 * a) as u8));
        let r = Rect::from_center_size(screen.center() + vec2(0.0, (1.0 - a) * 24.0 * s), vec2(1180.0 * s, 640.0 * s));
        crate::draw::glow(p, r, 28.0 * s, Color32::BLACK, 40.0 * s, 0.8 * a);
        p.rect_filled(r, CornerRadius::same((28.0 * s) as u8), Color32::from_rgb(22, 26, 36).gamma_multiply(a));
        p.rect_stroke(r, CornerRadius::same((28.0 * s) as u8), Stroke::new(1.0_f32, white(0.08 * a)), StrokeKind::Inside);
        let pad = 40.0 * s;
        let left = r.left() + pad;
        p.text(pos2(left, r.top() + pad), Align2::LEFT_TOP, "ПРОВЕРКА ГЕЙМПАДА", semibold(16.0 * s), BLUE.gamma_multiply(a));
        let name = self.state.as_ref().map_or("Геймпад не найден".to_string(), |st| clean_name(&st.name));
        p.text(pos2(left, r.top() + pad + 26.0 * s), Align2::LEFT_TOP, name, semibold(32.0 * s), white(a));

        // sticks: the raw position, the dead zone ring, the calibrated dot
        let raw = self.state.as_ref().map_or([0.0; 4], |st| st.raw);
        let cal = self.state.as_ref().map_or([0.0; 4], |st| st.cal);
        let ring = 110.0 * s;
        for (i, label) in ["Левый стик", "Правый стик"].iter().enumerate() {
            let c = pos2(left + ring + i as f32 * (ring * 2.0 + 60.0 * s), r.top() + 260.0 * s);
            p.circle_filled(c, ring, Color32::from_rgb(12, 15, 22).gamma_multiply(a));
            p.circle_stroke(c, ring, Stroke::new(2.0 * s, white(0.25 * a)));
            if let Some(k) = &self.cal {
                let dz = k[i * 2].dz.max(k[i * 2 + 1].dz);
                p.circle_filled(c, ring * dz, BLUE.gamma_multiply(0.18 * a));
                p.circle_stroke(c, ring * dz, Stroke::new(1.0 * s, BLUE.gamma_multiply(0.6 * a)));
            }
            let raw_pos = c + vec2(raw[i * 2], raw[i * 2 + 1]) * (ring - 26.0 * s);
            p.circle_filled(raw_pos, 24.0 * s, white(0.85 * a));
            let cal_pos = c + vec2(cal[i * 2], cal[i * 2 + 1]) * (ring - 26.0 * s);
            p.circle_filled(cal_pos, 7.0 * s, BLUE.gamma_multiply(a));
            p.text(pos2(c.x, c.y + ring + 18.0 * s), Align2::CENTER_TOP, *label, font(18.0 * s), white(0.7 * a));
            let pct = |v: f32| format!("{}{:.0}", if v >= 0.0 { "+" } else { "−" }, v.abs() * 100.0);
            let nums = format!("{} / {}   ·   {} / {}", pct(raw[i * 2]), pct(raw[i * 2 + 1]), pct(cal[i * 2]), pct(cal[i * 2 + 1]));
            p.text(pos2(c.x, c.y + ring + 44.0 * s), Align2::CENTER_TOP, nums, font(16.0 * s), white(0.5 * a));
        }

        // right column: triggers, buttons, status
        let rx = r.left() + 640.0 * s;
        let rw = r.right() - pad - rx;
        let mut y = r.top() + 140.0 * s;
        for (label, v) in [("LT", self.state.as_ref().map_or(0.0, |st| st.lt)), ("RT", self.state.as_ref().map_or(0.0, |st| st.rt))] {
            p.text(pos2(rx, y + 11.0 * s), Align2::LEFT_CENTER, label, semibold(18.0 * s), white(a));
            let bar = Rect::from_min_size(pos2(rx + 50.0 * s, y), vec2(rw - 50.0 * s, 22.0 * s));
            p.rect_filled(bar, CornerRadius::same((11.0 * s) as u8), white(0.1 * a));
            p.rect_filled(Rect::from_min_size(bar.min, vec2(bar.width() * v.clamp(0.0, 1.0), bar.height())), CornerRadius::same((11.0 * s) as u8), BLUE.gamma_multiply(a));
            y += 40.0 * s;
        }
        y += 10.0 * s;
        p.text(pos2(rx, y), Align2::LEFT_TOP, "Нажато", font(18.0 * s), white(0.6 * a));
        let pressed = self.state.as_ref().map(|st| st.pressed.join("  ")).filter(|t| !t.is_empty()).unwrap_or_else(|| "—".into());
        p.text(pos2(rx, y + 28.0 * s), Align2::LEFT_TOP, pressed, semibold(26.0 * s), white(a));
        y += 90.0 * s;

        let el = self.t0.elapsed();
        let (status, progress) = match self.phase {
            Phase::Rest => ("Шаг 1 из 2. Отпустите стики и не трогайте геймпад…".to_string(), Some(el.as_secs_f32() / REST.as_secs_f32())),
            Phase::Circle => ("Шаг 2 из 2. Медленно покрутите оба стика по кругу до упора…".to_string(), Some(el.as_secs_f32() / CIRCLE.as_secs_f32())),
            Phase::Idle => match (&self.cal, &self.state) {
                (Some(k), _) => (
                    format!(
                        "Откалиброван. Мёртвая зона: {:.0}% / {:.0}%",
                        k[0].dz.max(k[1].dz) * 100.0,
                        k[2].dz.max(k[3].dz) * 100.0
                    ),
                    None,
                ),
                (None, Some(_)) => ("Нажимайте кнопки и двигайте стики. Если меню «уплывает» само, откалибруйте стики.".into(), None),
                (None, None) => ("Подключите геймпад по USB или Bluetooth и нажмите любую кнопку.".into(), None),
            },
        };
        let g = p.layout(status, font(19.0 * s), white(0.85 * a), rw);
        let h = g.size().y;
        p.galley(pos2(rx, y), g, Color32::WHITE);
        if let Some(t) = progress {
            let bar = Rect::from_min_size(pos2(rx, y + h + 12.0 * s), vec2(rw, 6.0 * s));
            p.rect_filled(bar, CornerRadius::same(3), white(0.12 * a));
            p.rect_filled(Rect::from_min_size(bar.min, vec2(rw * t.clamp(0.0, 1.0), bar.height())), CornerRadius::same(3), BLUE.gamma_multiply(a));
        }

        // holds, with their progress
        let mut hx = left;
        let hy = r.bottom() - pad - 14.0 * s;
        for (i, text) in ["Удерживайте B / ○ — выйти", "Удерживайте Start — калибровка", "Удерживайте Y / △ — сбросить"].iter().enumerate() {
            let t = match self.holds[i] {
                Some(Some(at)) => (at.elapsed().as_secs_f32() / HOLD.as_secs_f32()).min(1.0),
                _ => 0.0,
            };
            let g = p.layout_no_wrap(text.to_string(), font(17.0 * s), white(0.75 * a));
            let w = g.size().x + 30.0 * s;
            let pill = Rect::from_min_size(pos2(hx, hy - 16.0 * s), vec2(w, 32.0 * s));
            p.rect_filled(pill, CornerRadius::same((16.0 * s) as u8), white(0.06 * a));
            if t > 0.0 {
                p.rect_filled(Rect::from_min_size(pill.min, vec2(w * t, pill.height())), CornerRadius::same((16.0 * s) as u8), BLUE.gamma_multiply(0.6 * a));
            }
            p.galley(pill.min + vec2(15.0 * s, (32.0 * s - g.size().y) / 2.0), g, Color32::WHITE);
            hx += w + 14.0 * s;
        }
    }
}

/// Centre, dead zone and reach from what was measured (the same numbers as the Electron build).
fn result(samples: &[Samples]) -> PadCal {
    let axes: Vec<AxisCal> = samples
        .iter()
        .map(|s| {
            let c = if s.n > 0 { (s.sum / s.n as f64) as f32 } else { 0.0 };
            let noise = s.vals.iter().fold(0f32, |m, v| m.max((v - c).abs()));
            let moved = s.hi - s.lo > 1.0;
            AxisCal {
                c,
                dz: (noise * 1.6 + 0.04).clamp(0.06, 0.3),
                lo: if moved { s.lo.min(-0.5) } else { -1.0 },
                hi: if moved { s.hi.max(0.5) } else { 1.0 },
            }
        })
        .collect();
    axes.try_into().unwrap_or([AxisCal { c: 0.0, dz: 0.1, lo: -1.0, hi: 1.0 }; 4])
}

fn clean_name(n: &str) -> String {
    let mut out = String::new();
    let mut depth = 0;
    for ch in n.chars() {
        match ch {
            '(' => depth += 1,
            ')' => depth = (depth - 1).max(0),
            _ if depth == 0 => out.push(ch),
            _ => {}
        }
    }
    let out = out.split_whitespace().collect::<Vec<_>>().join(" ");
    if out.is_empty() { "Геймпад".into() } else { out }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calibration_numbers() {
        // resting at -0.29 with a little jitter, then rolled to the edges
        let mut s = Samples { lo: 1.0, hi: -1.0, ..Default::default() };
        for v in [-0.29f32, -0.30, -0.28, -0.29] {
            s.sum += v as f64;
            s.n += 1;
            s.vals.push(v);
        }
        s.lo = -1.0;
        s.hi = 0.97;
        let still = Samples { lo: 0.0, hi: 0.0, n: 1, vals: vec![0.0], ..Default::default() };
        let k = result(&[s, still.clone(), still.clone(), still]);
        assert!((k[0].c + 0.29).abs() < 1e-3);
        assert!((k[0].dz - 0.06).abs() < 1e-3, "{}", k[0].dz);
        assert_eq!((k[0].lo, k[0].hi), (-1.0, 0.97));
        // an axis that never moved keeps the full range
        assert_eq!((k[1].lo, k[1].hi), (-1.0, 1.0));
        assert_eq!(clean_name("Xbox 360 Controller (XInput STANDARD GAMEPAD)"), "Xbox 360 Controller");
    }
}

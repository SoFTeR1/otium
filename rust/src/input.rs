//! Gamepad + keyboard turned into one stream of menu actions, with PS5-like key repeat.
//!
//! Sticks are read raw (gilrs' own dead zone is off: it rescales values and would hide a stick's
//! drift from the calibration) and then corrected with `settings.padCal`, the same data and
//! formula as the Electron build: per axis a resting centre `c`, a dead zone `dz` and the reach
//! `lo`/`hi`. Axes are kept in the browser convention there (Y grows downwards).

use gilrs::{Axis, Button, EventType, Gamepad, GamepadId, Gilrs, GilrsBuilder};
use serde_json::Value;
use std::collections::HashMap;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Nav {
    Up,
    Down,
    Left,
    Right,
    Ok,
    Back,
    Options,
    Add,
    Guide,
    /// Start / Menu (≡): the launcher's settings
    Menu,
}

const REPEAT_DELAY: Duration = Duration::from_millis(380);
const REPEAT_RATE: Duration = Duration::from_millis(110);
const STICK: f32 = 0.55;
/// dead zone for sticks nobody calibrated (about what gilrs used before)
const DEFAULT_DZ: f32 = 0.12;
/// Chromium's id for every XInput pad: the key the Electron build stored their calibration under
pub const XINPUT_ID: &str = "Xbox 360 Controller (XInput STANDARD GAMEPAD)";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PadKind {
    #[default]
    Xbox,
    PlayStation,
}

/// One axis' calibration, as `settings.padCal[id].axes[i]`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AxisCal {
    pub c: f32,
    pub dz: f32,
    pub lo: f32,
    pub hi: f32,
}

impl AxisCal {
    fn from_json(v: &Value) -> Option<Self> {
        let f = |k: &str, d: f64| v.get(k).and_then(Value::as_f64).unwrap_or(d) as f32;
        v.is_object().then(|| AxisCal { c: f("c", 0.0), dz: f("dz", 0.0), lo: f("lo", -1.0), hi: f("hi", 1.0) })
    }

    pub fn to_json(self) -> Value {
        let r = |x: f32, n: i32| {
            let k = 10f64.powi(n);
            (x as f64 * k).round() / k
        };
        serde_json::json!({ "c": r(self.c, 4), "dz": r(self.dz, 3), "lo": r(self.lo, 3), "hi": r(self.hi, 3) })
    }

    /// Centred and stretched back to -1 ..= 1 (Electron's calAxis).
    fn apply(&self, v: f32) -> f32 {
        let d = v - self.c;
        let out = if d >= 0.0 { d / (self.hi - self.c).max(0.2) } else { d / (self.c - self.lo).max(0.2) };
        out.clamp(-1.0, 1.0)
    }
}

pub type PadCal = [AxisCal; 4];

/// Reads `settings.padCal`: { id: { axes: [4 × {c, dz, lo, hi}], at } }.
pub fn parse_cal(v: Option<&Value>) -> HashMap<String, PadCal> {
    let mut out = HashMap::new();
    let Some(obj) = v.and_then(Value::as_object) else { return out };
    for (id, c) in obj {
        let Some(axes) = c.get("axes").and_then(Value::as_array) else { continue };
        let parsed: Vec<AxisCal> = axes.iter().filter_map(AxisCal::from_json).collect();
        if let Ok(a) = <[AxisCal; 4]>::try_from(parsed) {
            out.insert(id.clone(), a);
        }
    }
    out
}

/// One stick through its calibration and a round dead zone (Electron's padAxes).
pub fn stick(x: f32, y: f32, kx: Option<&AxisCal>, ky: Option<&AxisCal>) -> (f32, f32) {
    let (mut x, mut y) = (kx.map_or(x, |k| k.apply(x)), ky.map_or(y, |k| k.apply(y)));
    let dz = match (kx, ky) {
        (None, None) => DEFAULT_DZ,
        _ => kx.map_or(0.0, |k| k.dz).max(ky.map_or(0.0, |k| k.dz)),
    };
    let m = x.hypot(y);
    if m < dz {
        return (0.0, 0.0);
    }
    if dz > 0.0 && m > 0.0 {
        let s = ((m - dz) / (1.0 - dz)).min(1.0) / m;
        x *= s;
        y *= s;
    }
    (x, y)
}

/// What the pad test screen shows: the pad in hand right now.
#[derive(Debug, Clone, Default)]
pub struct PadState {
    /// the padCal key
    pub key: String,
    pub name: String,
    /// lx, ly, rx, ry as the pad reports them (Y down)
    pub raw: [f32; 4],
    /// the same after calibration and dead zone
    pub cal: [f32; 4],
    pub lt: f32,
    pub rt: f32,
    /// buttons held, by name
    pub pressed: Vec<&'static str>,
    pub back: bool,
    pub start: bool,
    pub north: bool,
}

pub struct Input {
    /// OTIUM is the window in front: otherwise pad presses belong to the game or the desktop and
    /// are read and dropped (the Guide button is caught globally elsewhere)
    pub focused: bool,
    gilrs: Option<Gilrs>,
    held: Option<(Nav, Instant, Instant)>,
    pub pad_kind: PadKind,
    pub pad_connected: bool,
    /// the on-screen keyboard is open: letters, Enter and Backspace go to it, not to navigation
    pub text_mode: bool,
    /// the pad test is open: every pad button is "being tested" and navigates nothing
    pub test_mode: bool,
    cal: HashMap<String, PadCal>,
    /// the pad that was used last
    active: Option<GamepadId>,
}

impl Input {
    pub fn new() -> Self {
        let gilrs = GilrsBuilder::new().with_default_filters(false).build().map_err(|e| eprintln!("gamepad: {e}")).ok();
        let mut me = Self {
            gilrs,
            held: None,
            pad_kind: PadKind::Xbox,
            pad_connected: false,
            text_mode: false,
            test_mode: false,
            focused: true,
            cal: HashMap::new(),
            active: None,
        };
        me.detect_pad();
        me
    }

    /// New `settings.padCal` (at start and after a calibration).
    pub fn set_cal(&mut self, v: Option<&Value>) {
        self.cal = parse_cal(v);
    }

    fn detect_pad(&mut self) {
        let Some(g) = &self.gilrs else { return };
        let first = g.gamepads().find(|(_, p)| p.is_connected());
        self.pad_connected = first.is_some();
        if let Some((_, p)) = first {
            let name = p.name().to_lowercase();
            self.pad_kind = if ["dualsense", "dualshock", "wireless controller", "playstation", "ps4", "ps5"]
                .iter()
                .any(|k| name.contains(k))
            {
                PadKind::PlayStation
            } else {
                PadKind::Xbox
            };
        }
    }

    /// The padCal key for a pad: the id Chromium gives it, so calibrations made in the Electron
    /// build apply here and the other way round.
    fn key(&self, p: &Gamepad) -> String {
        let ids = p.vendor_id().zip(p.product_id());
        if let Some((v, d)) = ids {
            let tag = format!("Vendor: {v:04x} Product: {d:04x}");
            if let Some(k) = self.cal.keys().find(|k| k.contains(&tag)) {
                return k.clone();
            }
        }
        let name = p.name().to_lowercase();
        let ps = ["dualsense", "dualshock", "wireless controller", "playstation"].iter().any(|k| name.contains(k));
        match ids {
            // Chromium reads Xbox-style pads through XInput, all under one id
            _ if !ps && (name.contains("xbox") || name.contains("xinput") || ids.is_some_and(|(v, _)| v == 0x045e)) => XINPUT_ID.to_string(),
            Some((v, d)) => format!("{} (STANDARD GAMEPAD Vendor: {v:04x} Product: {d:04x})", p.name()),
            None => p.name().to_string(),
        }
    }

    fn pad(&self) -> Option<Gamepad<'_>> {
        let g = self.gilrs.as_ref()?;
        if let Some(id) = self.active {
            if let Some(p) = g.connected_gamepad(id) {
                return Some(p);
            }
        }
        g.gamepads().find(|(_, p)| p.is_connected()).map(|(_, p)| p)
    }

    /// Both sticks of a pad: (raw, calibrated), Y down.
    fn sticks(&self, p: &Gamepad) -> ([f32; 4], [f32; 4]) {
        let raw = [
            p.value(Axis::LeftStickX),
            -p.value(Axis::LeftStickY),
            p.value(Axis::RightStickX),
            -p.value(Axis::RightStickY),
        ];
        let cal = self.cal.get(&self.key(p));
        let k = |i: usize| cal.map(|c| &c[i]);
        let (lx, ly) = stick(raw[0], raw[1], k(0), k(1));
        let (rx, ry) = stick(raw[2], raw[3], k(2), k(3));
        (raw, [lx, ly, rx, ry])
    }

    pub fn calibration(&self, key: &str) -> Option<&PadCal> {
        self.cal.get(key)
    }

    pub fn snapshot(&self) -> Option<PadState> {
        let p = self.pad()?;
        let (raw, cal) = self.sticks(&p);
        let trig = |b| p.button_data(b).map_or(0.0, |d| d.value());
        let names: [(Button, &'static str); 17] = [
            (Button::South, "A"),
            (Button::East, "B"),
            (Button::West, "X"),
            (Button::North, "Y"),
            (Button::LeftTrigger, "LB"),
            (Button::RightTrigger, "RB"),
            (Button::LeftTrigger2, "LT"),
            (Button::RightTrigger2, "RT"),
            (Button::Select, "View"),
            (Button::Start, "Start"),
            (Button::LeftThumb, "L3"),
            (Button::RightThumb, "R3"),
            (Button::DPadUp, "↑"),
            (Button::DPadDown, "↓"),
            (Button::DPadLeft, "←"),
            (Button::DPadRight, "→"),
            (Button::Mode, "Guide"),
        ];
        Some(PadState {
            key: self.key(&p),
            name: p.name().to_string(),
            raw,
            cal,
            lt: trig(Button::LeftTrigger2),
            rt: trig(Button::RightTrigger2),
            pressed: names.iter().filter(|(b, _)| p.is_pressed(*b)).map(|(_, n)| *n).collect(),
            back: p.is_pressed(Button::East),
            start: p.is_pressed(Button::Start),
            north: p.is_pressed(Button::North),
        })
    }

    pub fn poll(&mut self, ctx: &egui::Context) -> Vec<Nav> {
        let mut out = Vec::new();

        ctx.input(|i| {
            use egui::Key::*;
            for (key, nav) in [
                (ArrowUp, Nav::Up),
                (ArrowDown, Nav::Down),
                (ArrowLeft, Nav::Left),
                (ArrowRight, Nav::Right),
                (Enter, Nav::Ok),
                (Space, Nav::Ok),
                (Escape, Nav::Back),
                (Backspace, Nav::Back),
                (Q, Nav::Options),
                (N, Nav::Add),
                (Insert, Nav::Add),
                (Home, Nav::Guide),
            ] {
                let typing_key = !matches!(key, ArrowUp | ArrowDown | ArrowLeft | ArrowRight | Escape);
                if self.text_mode && typing_key {
                    continue;
                }
                if key_pressed(i, key) {
                    out.push(nav);
                }
            }
        });

        let Some(gilrs) = self.gilrs.as_mut() else { return out };
        let mut changed = false;
        let mut active = self.active;
        while let Some(ev) = gilrs.next_event() {
            match ev.event {
                EventType::ButtonPressed(b, _) => {
                    active = Some(ev.id);
                    if let Some(n) = button_nav(b).filter(|_| !self.test_mode && self.focused) {
                        out.push(n);
                    }
                }
                EventType::AxisChanged(..) => active = Some(ev.id),
                EventType::Connected | EventType::Disconnected => changed = true,
                _ => {}
            }
        }
        self.active = active;
        if changed {
            self.detect_pad();
        }
        if self.test_mode || !self.focused {
            self.held = None;
            return out;
        }

        // Directions come from the d-pad (buttons, or axes on some pads) or the calibrated left
        // stick, and repeat while held
        let dir = self.pad().and_then(|p| {
            let (_, c) = self.sticks(&p);
            let (x, y) = (c[0], c[1]);
            let dx = p.value(Axis::DPadX);
            let dy = p.value(Axis::DPadY);
            if p.is_pressed(Button::DPadUp) || dy > 0.5 || y < -STICK {
                Some(Nav::Up)
            } else if p.is_pressed(Button::DPadDown) || dy < -0.5 || y > STICK {
                Some(Nav::Down)
            } else if p.is_pressed(Button::DPadLeft) || dx < -0.5 || x < -STICK {
                Some(Nav::Left)
            } else if p.is_pressed(Button::DPadRight) || dx > 0.5 || x > STICK {
                Some(Nav::Right)
            } else {
                None
            }
        });
        let now = Instant::now();
        match (dir, self.held) {
            (None, _) => self.held = None,
            (Some(d), Some((h, start, last))) if d == h => {
                if now - start > REPEAT_DELAY && now - last > REPEAT_RATE {
                    out.push(d);
                    self.held = Some((d, start, now));
                }
            }
            (Some(d), _) => {
                out.push(d);
                self.held = Some((d, now, now));
            }
        }
        out
    }
}

/// A key press by its position on the keyboard, so letter shortcuts work in any layout
/// (with the Russian layout N types "т" and `Key::N` alone never fires).
pub fn key_pressed(i: &egui::InputState, key: egui::Key) -> bool {
    i.events.iter().any(|e| match e {
        egui::Event::Key { key: k, physical_key, pressed: true, modifiers, .. } => {
            (*k == key || *physical_key == Some(key)) && !modifiers.ctrl && !modifiers.alt
        }
        _ => false,
    })
}

fn button_nav(b: Button) -> Option<Nav> {
    Some(match b {
        Button::South => Nav::Ok,
        Button::East => Nav::Back,
        Button::North => Nav::Options,
        Button::Start => Nav::Menu,
        Button::West => Nav::Add,
        Button::Mode => Nav::Guide,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The calibration stored by the Electron build on this PC: both sticks rest at Y ≈ -0.29.
    #[test]
    fn drifting_stick_rests_at_zero() {
        let v: Value = serde_json::from_str(
            r#"{"Xbox 360 Controller (XInput STANDARD GAMEPAD)":{"axes":[{"c":0.0413,"dz":0.3,"lo":-1,"hi":1},{"c":-0.2927,"dz":0.3,"lo":-1,"hi":1},{"c":-0.0074,"dz":0.3,"lo":-1,"hi":1},{"c":-0.2808,"dz":0.3,"lo":-1,"hi":1}],"at":1793799122276}}"#,
        )
        .unwrap();
        let cal = parse_cal(Some(&v));
        let c = &cal[XINPUT_ID];
        // at rest: nothing
        assert_eq!(stick(0.0413, -0.2927, Some(&c[0]), Some(&c[1])), (0.0, 0.0));
        // pushed fully up and down: full reach both ways
        let (_, up) = stick(0.04, -1.0, Some(&c[0]), Some(&c[1]));
        let (_, down) = stick(0.04, 1.0, Some(&c[0]), Some(&c[1]));
        assert!(up < -0.99 && down > 0.99, "{up} {down}");
        // uncalibrated: the default dead zone only
        assert_eq!(stick(0.05, -0.05, None, None), (0.0, 0.0));
        // round trip of the stored numbers
        assert_eq!(c[1].to_json()["c"], serde_json::json!(-0.2927));
    }
}

/// The real pad, untouched: its padCal key and where its sticks rest before and after the
/// calibration stored in the real library. `cargo test -- --ignored real_pad --nocapture`
#[cfg(test)]
mod live {
    use super::*;

    #[test]
    #[ignore]
    fn real_pad() {
        let lib = crate::data::Library::load();
        let mut input = Input::new();
        input.set_cal(lib.settings.get("padCal"));
        let ctx = egui::Context::default();
        let mut worst = [0f32; 4];
        let mut worst_cal = [0f32; 4];
        let mut navs = Vec::new();
        for _ in 0..100 {
            ctx.begin_pass(Default::default());
            navs.extend(input.poll(&ctx));
            let _ = ctx.end_pass();
            if let Some(st) = input.snapshot() {
                for i in 0..4 {
                    worst[i] = worst[i].max(st.raw[i].abs());
                    worst_cal[i] = worst_cal[i].max(st.cal[i].abs());
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let Some(st) = input.snapshot() else { return println!("no pad connected") };
        println!("pad: {} -> key {:?}, calibrated: {}", st.name, st.key, input.calibration(&st.key).is_some());
        println!("at rest, largest |value| over 2 s  raw {worst:.3?}  calibrated {worst_cal:.3?}");
        println!("menu moves while untouched: {navs:?}");
    }
}

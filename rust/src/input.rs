//! Gamepad + keyboard turned into one stream of menu actions, with PS5-like key repeat.

use gilrs::{Axis, Button, EventType, Gilrs};
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
}

const REPEAT_DELAY: Duration = Duration::from_millis(380);
const REPEAT_RATE: Duration = Duration::from_millis(110);
const STICK: f32 = 0.55;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PadKind {
    #[default]
    Xbox,
    PlayStation,
}

pub struct Input {
    gilrs: Option<Gilrs>,
    held: Option<(Nav, Instant, Instant)>,
    pub pad_kind: PadKind,
    pub pad_connected: bool,
}

impl Input {
    pub fn new() -> Self {
        let gilrs = Gilrs::new().map_err(|e| eprintln!("gamepad: {e}")).ok();
        let mut me = Self { gilrs, held: None, pad_kind: PadKind::Xbox, pad_connected: false };
        me.detect_pad();
        me
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
                if key_pressed(i, key) {
                    out.push(nav);
                }
            }
        });

        let Some(gilrs) = self.gilrs.as_mut() else { return out };
        let mut changed = false;
        while let Some(ev) = gilrs.next_event() {
            match ev.event {
                EventType::ButtonPressed(b, _) => {
                    if let Some(n) = button_nav(b) {
                        out.push(n);
                    }
                }
                EventType::Connected | EventType::Disconnected => changed = true,
                _ => {}
            }
        }
        if changed {
            self.detect_pad();
        }
        let Some(gilrs) = self.gilrs.as_ref() else { return out };

        // Directions come from the d-pad or the left stick and repeat while held
        let dir = gilrs.gamepads().filter(|(_, p)| p.is_connected()).find_map(|(_, p)| {
            let x = p.value(Axis::LeftStickX);
            let y = p.value(Axis::LeftStickY);
            if p.is_pressed(Button::DPadUp) || y > STICK {
                Some(Nav::Up)
            } else if p.is_pressed(Button::DPadDown) || y < -STICK {
                Some(Nav::Down)
            } else if p.is_pressed(Button::DPadLeft) || x < -STICK {
                Some(Nav::Left)
            } else if p.is_pressed(Button::DPadRight) || x > STICK {
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
        Button::North | Button::Start => Nav::Options,
        Button::West => Nav::Add,
        Button::Mode => Nav::Guide,
        _ => return None,
    })
}

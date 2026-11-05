//! The gamepad inside a Media service window (a browser OTIUM opened): OTIUM itself is behind it,
//! so the pad is read here through XInput and turned into the mouse and keys the page expects.
//!
//! Mouse mode (Кинопоиск, Twitch, YouTube on the web): left stick — pointer, right stick — scroll,
//! A — click, X — right click, B — Esc, Y — back, d-pad — arrows.
//! Keys mode (YouTube for TV): d-pad / left stick — arrows, A — Enter, B — Esc.
//! The Guide button still brings OTIUM back (the watcher in `watch` handles it).

use crate::input::{AxisCal, PadCal};
use crate::watch::{load_get_state_ex, XState};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP,
    MOUSEEVENTF_HWHEEL, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP,
    MOUSEEVENTF_WHEEL, MOUSEINPUT, MOUSE_EVENT_FLAGS, VIRTUAL_KEY,
};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Mouse,
    Keys,
}

/// Browsers of the service windows the pad drives (by process: the browser's own popups, like its
/// "press F11 to exit" bubble, count too), and whether it does at all (Настройки → Геймпад в Медиа).
#[derive(Default)]
pub struct Shared {
    pub windows: Mutex<Vec<(u32, Mode)>>,
    pub on: AtomicBool,
    /// the XInput pad's calibration (settings.padCal), so a stick that rests off-centre doesn't drift
    pub cal: Mutex<Option<PadCal>>,
}

impl Shared {
    pub fn set_cal(&self, all: &std::collections::HashMap<String, PadCal>) {
        *self.cal.lock().unwrap() = all.get(crate::input::XINPUT_ID).copied();
    }
}

const A: u16 = 0x1000;
const B: u16 = 0x2000;
const X: u16 = 0x4000;
const Y: u16 = 0x8000;
const UP: u16 = 0x0001;
const DOWN: u16 = 0x0002;
const LEFT: u16 = 0x0004;
const RIGHT: u16 = 0x0008;

const VK_LEFT: u16 = 0x25;
const VK_UP: u16 = 0x26;
const VK_RIGHT: u16 = 0x27;
const VK_DOWN: u16 = 0x28;
const VK_RETURN: u16 = 0x0D;
const VK_ESCAPE: u16 = 0x1B;
const VK_BROWSER_BACK: u16 = 0xA6;

/// dead zone of a pad nobody calibrated (larger than the menus': a drifting stick would move the pointer)
const DEAD: AxisCal = AxisCal { c: 0.0, dz: 0.24, lo: -1.0, hi: 1.0 };
/// pointer speed at full tilt, px per second (the curve is quadratic below that)
const SPEED: f32 = 1500.0;
const SCROLL: f32 = 1400.0;
const REPEAT_DELAY: Duration = Duration::from_millis(380);
const REPEAT_RATE: Duration = Duration::from_millis(110);

/// Both sticks through the calibration and a round dead zone, Y pointing down: (lx, ly, rx, ry).
fn sticks(s: &XState, cal: Option<&PadCal>) -> [f32; 4] {
    let n = |v: i16| (v as f32 / 32767.0).clamp(-1.0, 1.0);
    let k = |i: usize| Some(cal.map_or(&DEAD, |c| &c[i]));
    let (lx, ly) = crate::input::stick(n(s.lx), -n(s.ly), k(0), k(1));
    let (rx, ry) = crate::input::stick(n(s.rx), -n(s.ry), k(2), k(3));
    [lx, ly, rx, ry]
}

fn mouse(flags: MOUSE_EVENT_FLAGS, dx: i32, dy: i32, data: i32) {
    let i = INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 { mi: MOUSEINPUT { dx, dy, mouseData: data as u32, dwFlags: flags, time: 0, dwExtraInfo: 0 } },
    };
    unsafe { SendInput(&[i], std::mem::size_of::<INPUT>() as i32) };
}

fn key(vk: u16, down: bool) {
    let mut flags = if down { KEYBD_EVENT_FLAGS(0) } else { KEYEVENTF_KEYUP };
    if matches!(vk, VK_LEFT | VK_UP | VK_RIGHT | VK_DOWN) {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    let i = INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: VIRTUAL_KEY(vk), wScan: 0, dwFlags: flags, time: 0, dwExtraInfo: 0 } },
    };
    unsafe { SendInput(&[i], std::mem::size_of::<INPUT>() as i32) };
}

fn tap(vk: u16) {
    key(vk, true);
    key(vk, false);
}

/// A direction held on the d-pad or the stick: one arrow press, then repeats like a keyboard.
#[derive(Default)]
struct Repeat {
    held: Option<(u16, Instant, Instant)>,
}

impl Repeat {
    fn update(&mut self, vk: Option<u16>) {
        let now = Instant::now();
        match (vk, self.held) {
            (None, _) => self.held = None,
            (Some(v), Some((h, since, last))) if v == h => {
                if now - since >= REPEAT_DELAY && now - last >= REPEAT_RATE {
                    tap(v);
                    self.held = Some((h, since, now));
                }
            }
            (Some(v), _) => {
                tap(v);
                self.held = Some((v, now, now));
            }
        }
    }
}

#[derive(Default)]
struct State {
    prev: u16,
    left_down: bool,
    right_down: bool,
    frac: (f32, f32),
    wheel: (f32, f32),
    arrows: Repeat,
}

impl State {
    /// Lets go of whatever is held (the window lost focus or the pad went away).
    fn release(&mut self) {
        if self.left_down {
            mouse(MOUSEEVENTF_LEFTUP, 0, 0, 0);
        }
        if self.right_down {
            mouse(MOUSEEVENTF_RIGHTUP, 0, 0, 0);
        }
        *self = State::default();
    }

    fn step(&mut self, s: &XState, cal: Option<&PadCal>, mode: Mode, dt: f32) {
        let b = s.buttons;
        let pressed = |m: u16| b & m != 0 && self.prev & m == 0;
        let [lx, ly, rx, ry] = sticks(s, cal);

        // arrows: the d-pad, and in keys mode the left stick too
        let dir = if b & UP != 0 {
            Some(VK_UP)
        } else if b & DOWN != 0 {
            Some(VK_DOWN)
        } else if b & LEFT != 0 {
            Some(VK_LEFT)
        } else if b & RIGHT != 0 {
            Some(VK_RIGHT)
        } else if mode == Mode::Keys && (lx.abs() > 0.4 || ly.abs() > 0.4) {
            Some(if lx.abs() > ly.abs() {
                if lx > 0.0 { VK_RIGHT } else { VK_LEFT }
            } else if ly < 0.0 {
                VK_UP
            } else {
                VK_DOWN
            })
        } else {
            None
        };
        self.arrows.update(dir);

        if pressed(B) {
            tap(VK_ESCAPE);
        }
        match mode {
            Mode::Keys => {
                if pressed(A) {
                    tap(VK_RETURN);
                }
            }
            Mode::Mouse => {
                // pointer
                let m = (lx * lx + ly * ly).sqrt().min(1.0);
                if m > 0.0 {
                    let v = SPEED * m * m * dt / m;
                    self.frac.0 += lx * v;
                    self.frac.1 += ly * v;
                    let (dx, dy) = (self.frac.0.trunc(), self.frac.1.trunc());
                    if dx != 0.0 || dy != 0.0 {
                        mouse(MOUSEEVENTF_MOVE, dx as i32, dy as i32, 0);
                        self.frac.0 -= dx;
                        self.frac.1 -= dy;
                    }
                }
                // scroll
                // stick down scrolls down (a negative wheel delta)
                self.wheel.0 += rx * rx.abs() * SCROLL * dt;
                self.wheel.1 -= ry * ry.abs() * SCROLL * dt;
                if self.wheel.1.abs() >= 20.0 {
                    mouse(MOUSEEVENTF_WHEEL, 0, 0, self.wheel.1 as i32);
                    self.wheel.1 = 0.0;
                }
                if self.wheel.0.abs() >= 20.0 {
                    mouse(MOUSEEVENTF_HWHEEL, 0, 0, self.wheel.0 as i32);
                    self.wheel.0 = 0.0;
                }
                if rx == 0.0 {
                    self.wheel.0 = 0.0;
                }
                if ry == 0.0 {
                    self.wheel.1 = 0.0;
                }
                // buttons follow the pad: hold A to drag
                let a = b & A != 0;
                if a != self.left_down {
                    mouse(if a { MOUSEEVENTF_LEFTDOWN } else { MOUSEEVENTF_LEFTUP }, 0, 0, 0);
                    self.left_down = a;
                }
                let x = b & X != 0;
                if x != self.right_down {
                    mouse(if x { MOUSEEVENTF_RIGHTDOWN } else { MOUSEEVENTF_RIGHTUP }, 0, 0, 0);
                    self.right_down = x;
                }
                if pressed(Y) {
                    tap(VK_BROWSER_BACK);
                }
            }
        }
        self.prev = b;
    }
}

pub fn start(shared: Arc<Shared>) {
    let Some(get) = load_get_state_ex() else { return };
    std::thread::Builder::new()
        .name("padmouse".into())
        .spawn(move || {
            let mut st = State::default();
            let mut active = false;
            let mut last = Instant::now();
            loop {
                let fg = crate::win::window_pid(crate::win::foreground());
                let mode = if shared.on.load(Ordering::Relaxed) && fg != 0 {
                    shared.windows.lock().unwrap().iter().find(|(p, _)| *p == fg).map(|(_, m)| *m)
                } else {
                    None
                };
                let Some(mode) = mode else {
                    if active {
                        st.release();
                        active = false;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                    continue;
                };
                // the first pad that answers
                let pad = (0..4u32).find_map(|i| {
                    let mut s = XState::default();
                    (unsafe { get(i, &mut s) } == 0).then_some(s)
                });
                let now = Instant::now();
                let dt = (now - last).as_secs_f32().min(0.05);
                last = now;
                match pad {
                    Some(s) => {
                        if !active {
                            // buttons already down when the window came up (the A that opened it) don't count
                            st.prev = s.buttons;
                            active = true;
                        }
                        let cal = *shared.cal.lock().unwrap();
                        st.step(&s, cal.as_ref(), mode, dt);
                    }
                    None if active => {
                        st.release();
                        active = false;
                    }
                    None => {}
                }
                std::thread::sleep(Duration::from_millis(8));
            }
        })
        .ok();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st(lx: i16, ly: i16) -> XState {
        XState { lx, ly, ..Default::default() }
    }

    #[test]
    fn dead_zone() {
        assert_eq!(sticks(&st(3000, -7000), None)[..2], [0.0, 0.0]);
        let full = sticks(&st(32767, 0), None);
        assert!((full[0] - 1.0).abs() < 1e-3);
        // XInput's Y points up; ours points down
        assert!(sticks(&st(0, 20000), None)[1] < -0.3);
    }

    #[test]
    fn calibration_hides_drift() {
        // a stick resting at 0.29 down with its calibration: no movement
        let c = AxisCal { c: 0.29, dz: 0.3, lo: -1.0, hi: 1.0 };
        let z = AxisCal { c: 0.0, dz: 0.3, lo: -1.0, hi: 1.0 };
        let cal = [z, c, z, z];
        let rest = st(0, (-0.29 * 32767.0) as i16);
        assert_eq!(sticks(&rest, Some(&cal))[..2], [0.0, 0.0]);
        assert!(sticks(&rest, None)[1] > 0.0);
    }
}

//! Interface sounds, synthesized like the Electron build did with WebAudio: short tones for moving
//! and choosing, and the splash chord (a Dmaj9 pad opening through a filter, three bells, a hall).
//! Everything is rendered to buffers once and played on a background thread.

use std::sync::mpsc::{channel, Sender};
use std::sync::OnceLock;

const RATE: u32 = 44_100;
/// the splash chord at "Средне"; Настройки → Звук заставки scales it (`play_gain`)
const SPLASH_GAIN: f32 = 1.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Sfx {
    Move,
    Ok,
    Back,
    Launch,
    Err,
    Splash,
    /// incoming call: a friendly marimba phrase
    Ring,
    /// outgoing ringback: two calm notes
    Ringback,
    /// call connected: rising sparkle
    Connect,
    /// call ended: gentle fall
    End,
    MicOn,
    MicOff,
}

static TX: OnceLock<Sender<(Sfx, f32)>> = OnceLock::new();

/// Plays a sound (does nothing if there is no audio device).
pub fn play(s: Sfx) {
    play_gain(s, 1.0);
}

/// Plays a sound `gain` times as loud.
pub fn play_gain(s: Sfx, gain: f32) {
    let tx = TX.get_or_init(|| {
        let (tx, rx) = channel::<(Sfx, f32)>();
        std::thread::Builder::new()
            .name("sound".into())
            .spawn(move || {
                let Ok((_stream, handle)) = rodio::OutputStream::try_default() else { return };
                let mut cache: std::collections::HashMap<Sfx, std::sync::Arc<Vec<f32>>> = Default::default();
                while let Ok((s, gain)) = rx.recv() {
                    let buf = cache.entry(s).or_insert_with(|| std::sync::Arc::new(render(s))).clone();
                    let samples: Vec<f32> = buf.iter().map(|v| (v * gain).clamp(-1.0, 1.0)).collect();
                    let src = rodio::buffer::SamplesBuffer::new(2, RATE, samples);
                    let _ = handle.play_raw(rodio::Source::convert_samples::<f32>(src));
                }
            })
            .ok();
        tx
    });
    let _ = tx.send((s, gain));
}

#[derive(Clone, Copy)]
enum Wave {
    Sine,
    Triangle,
    Square,
}

fn osc(w: Wave, phase: f32) -> f32 {
    let p = phase.fract();
    match w {
        Wave::Sine => (p * std::f32::consts::TAU).sin(),
        Wave::Triangle => 4.0 * (p - 0.5).abs() - 1.0,
        Wave::Square => if p < 0.5 { 0.6 } else { -0.6 },
    }
}

/// A tone that starts at `vol` and decays exponentially to silence over `dur` (WebAudio's
/// setValueAtTime + exponentialRampToValueAtTime), mixed into a mono buffer.
fn tone(buf: &mut Vec<f32>, freq: f32, dur: f32, vol: f32, w: Wave, delay: f32) {
    let start = (delay * RATE as f32) as usize;
    let n = (dur * RATE as f32) as usize;
    if buf.len() < start + n {
        buf.resize(start + n, 0.0);
    }
    let k = (0.0001f32 / vol).ln() / n as f32;
    for i in 0..n {
        let t = i as f32 / RATE as f32;
        let env = vol * (k * i as f32).exp();
        // 2 ms fade-in against clicks
        let fade = (i as f32 / (0.002 * RATE as f32)).min(1.0);
        buf[start + i] += osc(w, freq * t) * env * fade;
    }
}

fn stereo(mono: Vec<f32>) -> Vec<f32> {
    mono.into_iter().flat_map(|v| [v, v]).collect()
}

fn render(s: Sfx) -> Vec<f32> {
    let mut b = Vec::new();
    match s {
        Sfx::Move => tone(&mut b, 1400.0, 0.035, 0.025, Wave::Sine, 0.0),
        Sfx::Ok => {
            tone(&mut b, 880.0, 0.06, 0.04, Wave::Sine, 0.0);
            tone(&mut b, 1320.0, 0.09, 0.035, Wave::Sine, 0.05);
        }
        Sfx::Back => tone(&mut b, 520.0, 0.08, 0.035, Wave::Sine, 0.0),
        Sfx::Launch => {
            tone(&mut b, 440.0, 0.25, 0.05, Wave::Triangle, 0.0);
            tone(&mut b, 660.0, 0.3, 0.04, Wave::Triangle, 0.12);
            tone(&mut b, 990.0, 0.5, 0.03, Wave::Sine, 0.24);
        }
        Sfx::Err => tone(&mut b, 200.0, 0.18, 0.05, Wave::Square, 0.0),
        Sfx::Splash => return splash(),
        Sfx::Ring | Sfx::Ringback | Sfx::Connect | Sfx::End | Sfx::MicOn | Sfx::MicOff => return call_sound(s),
    }
    // the Electron build ran these through the browser's output at about this level
    for v in &mut b {
        *v *= 2.5;
    }
    stereo(b)
}

/// MIDI note -> Hz
fn note(n: f32) -> f32 {
    440.0 * 2f32.powf((n - 69.0) / 12.0)
}

/// A bell: sine partials with a soft attack and an exponential ring, mixed into `buf`.
fn bell(buf: &mut Vec<f32>, midi: f32, at: f32, dur: f32, vol: f32, attack: f32, partials: &[(f32, f32)]) {
    let f = note(midi);
    let start = (at * RATE as f32) as usize;
    let len = ((dur + 0.05) * RATE as f32) as usize;
    if buf.len() < start + len {
        buf.resize(start + len, 0.0);
    }
    for &(mult, amp) in partials {
        let d = dur / mult.sqrt();
        for i in 0..len {
            let t = i as f32 / RATE as f32;
            let env = if t < attack { vol * amp * t / attack } else { vol * amp * (0.0001f32 / (vol * amp)).powf(((t - attack) / d).min(1.0)) };
            buf[start + i] += (std::f32::consts::TAU * f * mult * t).sin() * env;
        }
    }
}

/// Call sounds, after the Electron build's little bell synth (with its short echo).
fn call_sound(s: Sfx) -> Vec<f32> {
    let mut b = Vec::new();
    let marimba: &[(f32, f32)] = &[(1.0, 1.0), (3.99, 0.22), (9.9, 0.04)];
    let plain: &[(f32, f32)] = &[(1.0, 1.0), (2.01, 0.28), (3.98, 0.08), (5.4, 0.03)];
    let soft: &[(f32, f32)] = &[(1.0, 1.0), (2.0, 0.12)];
    match s {
        Sfx::Ring => {
            for (n, at) in [(79.0, 0.0), (83.0, 0.14), (86.0, 0.28), (83.0, 0.56), (86.0, 0.7)] {
                bell(&mut b, n, at, 0.9, 0.13, 0.008, marimba);
            }
        }
        Sfx::Ringback => {
            bell(&mut b, 69.0, 0.0, 1.6, 0.07, 0.09, soft);
            bell(&mut b, 76.0, 0.42, 1.6, 0.07, 0.09, soft);
        }
        Sfx::Connect => {
            for (n, at) in [(76.0, 0.0), (80.0, 0.07), (83.0, 0.14), (88.0, 0.21)] {
                bell(&mut b, n, at, 1.1, 0.1, 0.008, plain);
            }
        }
        Sfx::End => {
            bell(&mut b, 76.0, 0.0, 0.9, 0.09, 0.008, plain);
            bell(&mut b, 69.0, 0.16, 1.2, 0.09, 0.008, plain);
        }
        Sfx::MicOn | Sfx::MicOff => bell(&mut b, if s == Sfx::MicOn { 84.0 } else { 77.0 }, 0.0, 0.25, 0.06, 0.008, &[(1.0, 1.0)]),
        _ => {}
    }
    // short feedback echo for some air
    let d = (0.18 * RATE as f32) as usize;
    let n = b.len();
    b.resize(n + d * 4, 0.0);
    let mut wet = vec![0f32; b.len()];
    for i in d..b.len() {
        wet[i] = b[i - d] * 0.35 + wet[i - d] * 0.22;
    }
    for (x, w) in b.iter_mut().zip(wet) {
        *x = (*x + w).clamp(-1.0, 1.0);
    }
    stereo(b)
}

/// When the bells ring, in seconds (the Rust splash shows the word OTIUM at about this point).
pub const SPLASH_BELLS: f32 = 1.0;

fn splash() -> Vec<f32> {
    let secs = 7.5;
    let n = (secs * RATE as f32) as usize;
    let mut dry = vec![0.0f32; n];
    let t0 = 0.05;

    // pad: two slightly detuned voices per note through a lowpass that opens up, then closes
    let notes = [73.42f32, 146.83, 220.0, 369.99, 554.37, 659.25]; // D2 D3 A3 F#4 C#5 E5
    let mut pad = vec![0.0f32; n];
    for (i, f) in notes.iter().enumerate() {
        for (w, cents, lvl) in [(Wave::Sine, -7.0f32, 0.6f32), (Wave::Triangle, 6.0, 0.32)] {
            let freq = f * 2f32.powf(cents / 1200.0);
            let lvl = lvl * if i == 0 { 0.9 } else if *f > 500.0 { 0.45 } else { 1.0 };
            let start = ((t0 + i as f32 * 0.06) * RATE as f32) as usize;
            for (j, out) in pad.iter_mut().enumerate().skip(start) {
                let t = (j - start) as f32 / RATE as f32;
                *out += osc(w, freq * t) * lvl;
            }
        }
    }
    let mut y = 0.0f32;
    for (j, v) in pad.iter().enumerate() {
        let t = j as f32 / RATE as f32 - t0;
        // cutoff 220 Hz -> 4200 Hz over 2.1 s, then down to 1400 Hz by 5 s
        let cutoff = if t < 2.1 { 220.0 * (4200.0f32 / 220.0).powf((t / 2.1).max(0.0)) } else { 4200.0 * (1400.0f32 / 4200.0).powf(((t - 2.1) / 2.9).min(1.0)) };
        let a = 1.0 - (-std::f32::consts::TAU * cutoff / RATE as f32).exp();
        y += a * (v - y);
        // gain: exponential rise to 0.085 over 1.5 s, held, released from 2.9 s with tau 0.8
        let g = if t < 0.0 {
            0.0
        } else if t < 1.5 {
            0.0001 * (0.085f32 / 0.0001).powf(t / 1.5)
        } else if t < 2.9 {
            0.085
        } else {
            0.085 * (-(t - 2.9) / 0.8).exp()
        };
        dry[j] += y * g;
    }

    // bells: sine + inharmonic partials, quick attack, long ring
    let bell = |dry: &mut Vec<f32>, f: f32, at: f32, vol: f32| {
        let start = ((t0 + at) * RATE as f32) as usize;
        let len = (3.0 * RATE as f32) as usize;
        for i in 0..len.min(dry.len().saturating_sub(start)) {
            let t = i as f32 / RATE as f32;
            let env = if t < 0.008 { 0.0001 * (vol / 0.0001).powf(t / 0.008) } else { vol * (0.0001f32 / vol).powf(((t - 0.008) / 2.792).min(1.0)) };
            let s: f32 = [(1.0, 1.0), (2.76, 0.18), (5.4, 0.05)].iter().map(|(m, l)| (std::f32::consts::TAU * f * m * t).sin() * l).sum();
            dry[start + i] += s * env;
        }
    };
    bell(&mut dry, 1318.5, SPLASH_BELLS, 0.05);
    bell(&mut dry, 1760.0, SPLASH_BELLS + 0.13, 0.035);
    bell(&mut dry, 2217.5, SPLASH_BELLS + 0.26, 0.022);

    // a big room: Schroeder reverb (parallel combs into allpasses), slightly different per side
    let wet_l = reverb(&dry, &[1557, 1617, 1491, 1422], &[225, 556]);
    let wet_r = reverb(&dry, &[1580, 1640, 1514, 1445], &[248, 579]);
    let mut out = Vec::with_capacity(n * 2);
    for i in 0..n {
        out.push(0.8 * (dry[i] + 0.6 * wet_l[i]) * SPLASH_GAIN);
        out.push(0.8 * (dry[i] + 0.6 * wet_r[i]) * SPLASH_GAIN);
    }
    out
}

fn reverb(x: &[f32], combs: &[usize], allpasses: &[usize]) -> Vec<f32> {
    let scale = 2.0; // the classic delays are for 22 kHz-ish rooms; stretch them for a hall
    let mut sum = vec![0.0f32; x.len()];
    for &d in combs {
        let d = (d as f32 * scale) as usize;
        let mut buf = vec![0.0f32; d];
        let mut filt = 0.0f32;
        for (i, s) in sum.iter_mut().enumerate() {
            let out = buf[i % d];
            filt = out * 0.8 + filt * 0.2; // damping
            buf[i % d] = x[i] + filt * 0.86;
            *s += out * 0.25;
        }
    }
    for &d in allpasses {
        let d = (d as f32 * scale) as usize;
        let mut buf = vec![0.0f32; d];
        for (i, s) in sum.iter_mut().enumerate() {
            let delayed = buf[i % d];
            let y = -*s + delayed;
            buf[i % d] = *s + delayed * 0.5;
            *s = y;
        }
    }
    sum
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_without_clipping() {
        for s in [Sfx::Move, Sfx::Ok, Sfx::Back, Sfx::Launch, Sfx::Err] {
            let b = render(s);
            assert!(!b.is_empty());
            assert!(b.iter().all(|v| v.abs() <= 1.0), "{s:?} clips");
        }
    }

    #[test]
    fn splash_renders() {
        let b = splash();
        assert_eq!(b.len(), (7.5 * RATE as f32) as usize * 2);
        let peak = b.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        assert!(peak > 0.01 && peak <= 1.0, "peak {peak}");
    }
}

/// `settings.splashSound` = "off" | "low" | "mid" | "high" -> gain of the splash chord.
pub fn splash_gain(setting: &str) -> f32 {
    match setting {
        "off" => 0.0,
        "low" => 0.6,
        "high" => 1.8,
        _ => 1.1,
    }
}

pub fn level_name(setting: &str) -> &'static str {
    match setting {
        "off" => "Выкл",
        "low" => "Тихо",
        "high" => "Громко",
        _ => "Средне",
    }
}

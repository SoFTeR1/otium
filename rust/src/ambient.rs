//! Background music of the launcher: the track built into the exe (`assets/ambient.ogg` or
//! `.mp3`, see build.rs), looped, faded in while OTIUM is the window in front and faded out —
//! then paused, so it continues where it was — whenever it isn't (a game, the desktop, the tray),
//! during calls and while other music is playing in Windows.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

static BYTES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/ambient.bin"));
static WANT: AtomicBool = AtomicBool::new(false);
/// volume * 1000
static VOLUME: AtomicU32 = AtomicU32::new(300);
static STARTED: OnceLock<()> = OnceLock::new();

/// Whether this build has music in it.
pub fn available() -> bool {
    !BYTES.is_empty()
}

/// What the player should do now: play at `volume` (0..=1) or fade out.
pub fn set(want: bool, volume: f32) {
    VOLUME.store((volume.clamp(0.0, 1.0) * 1000.0) as u32, Ordering::Relaxed);
    WANT.store(want && volume > 0.0, Ordering::Relaxed);
    if want && available() {
        STARTED.get_or_init(start);
    }
}

fn start() {
    std::thread::Builder::new()
        .name("ambient".into())
        .spawn(|| {
            let Ok((_stream, handle)) = rodio::OutputStream::try_default() else { return };
            let Ok(sink) = rodio::Sink::try_new(&handle) else { return };
            let Ok(src) = rodio::Decoder::new_looped(std::io::Cursor::new(BYTES)) else {
                eprintln!("ambient: the built-in track can't be decoded ({})", env!("OTIUM_AMBIENT_KIND"));
                return;
            };
            sink.set_volume(0.0);
            sink.append(src);
            sink.pause();
            let mut cur = 0.0f32;
            let tick = Duration::from_millis(30);
            loop {
                let target = if WANT.load(Ordering::Relaxed) { VOLUME.load(Ordering::Relaxed) as f32 / 1000.0 } else { 0.0 };
                // about 1.5 s to fade in, 0.6 s to fade out
                let step = if target > cur { 0.02 } else { 0.05 };
                cur = if (target - cur).abs() <= step { target } else { cur + step.copysign(target - cur) };
                // the ear hears loudness roughly as the square of the amplitude
                sink.set_volume(cur * cur);
                if cur <= 0.0 && !sink.is_paused() {
                    sink.pause();
                } else if cur > 0.0 && sink.is_paused() {
                    sink.play();
                }
                std::thread::sleep(tick);
            }
        })
        .ok();
}

#[cfg(test)]
mod tests {
    /// The built-in track (if any) decodes and has sound in it.
    #[test]
    fn built_in_track_decodes() {
        if !super::available() {
            return;
        }
        let dec = rodio::Decoder::new(std::io::Cursor::new(super::BYTES)).expect("decodes");
        let (ch, rate) = (rodio::Source::channels(&dec), rodio::Source::sample_rate(&dec));
        let samples: Vec<i16> = dec.take(rate as usize * ch as usize * 20).collect();
        let peak = samples.iter().map(|v| v.unsigned_abs()).max().unwrap_or(0);
        println!("{} ({ch} ch, {rate} Hz): first 20 s peak {peak}", env!("OTIUM_AMBIENT_KIND"));
        assert!(samples.len() > rate as usize && peak > 100, "silent or empty");
    }
}

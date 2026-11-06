//! What the speakers play, through WASAPI loopback, as stereo 16-bit at 48 kHz. Follows the default
//! output: moving the sound to the TV moves the recording with it.

use super::Shared;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};
use windows::core::Result;
use windows::Win32::Media::Audio::*;
use windows::Win32::Media::KernelStreaming::WAVE_FORMAT_EXTENSIBLE;
use windows::Win32::Media::Multimedia::{KSDATAFORMAT_SUBTYPE_IEEE_FLOAT, WAVE_FORMAT_IEEE_FLOAT};
use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CoTaskMemFree, CLSCTX_ALL, COINIT_MULTITHREADED};

pub fn start(shared: Arc<Shared>) {
    std::thread::Builder::new()
        .name("clip-sound".into())
        .spawn(move || unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let Ok(en) = CoCreateInstance::<_, IMMDeviceEnumerator>(&MMDeviceEnumerator, None, CLSCTX_ALL) else { return };
            loop {
                if !shared.want.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(200));
                    continue;
                }
                if let Err(e) = record(&shared, &en) {
                    eprintln!("clips: sound: {e}");
                    std::thread::sleep(Duration::from_secs(1));
                }
            }
        })
        .ok();
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Sample {
    F32,
    I16,
    I32,
}

unsafe fn device_id(d: &IMMDevice) -> String {
    d.GetId().ok().map_or(String::new(), |p| {
        let s = p.to_string().unwrap_or_default();
        CoTaskMemFree(Some(p.0 as *const _));
        s
    })
}

unsafe fn record(shared: &Shared, en: &IMMDeviceEnumerator) -> Result<()> {
    let device = en.GetDefaultAudioEndpoint(eRender, eConsole)?;
    let id = device_id(&device);
    let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
    let fmt = client.GetMixFormat()?;
    let (channels, rate, kind) = {
        let f = &*fmt;
        let tag = f.wFormatTag as u32;
        let float = if tag == WAVE_FORMAT_EXTENSIBLE {
            let sub = std::ptr::addr_of!((*(fmt as *const WAVEFORMATEXTENSIBLE)).SubFormat).read_unaligned();
            sub == KSDATAFORMAT_SUBTYPE_IEEE_FLOAT
        } else {
            tag == WAVE_FORMAT_IEEE_FLOAT
        };
        let kind = match (float, f.wBitsPerSample) {
            (true, _) => Sample::F32,
            (false, 16) => Sample::I16,
            _ => Sample::I32,
        };
        (f.nChannels as usize, f.nSamplesPerSec, kind)
    };
    let started = client.Initialize(AUDCLNT_SHAREMODE_SHARED, AUDCLNT_STREAMFLAGS_LOOPBACK, 10_000_000, 0, fmt, None);
    CoTaskMemFree(Some(fmt as *const _));
    started?;
    let capture: IAudioCaptureClient = client.GetService()?;
    client.Start()?;

    let mut conv = Convert::new(channels, rate, kind);
    let mut checked = Instant::now();
    let mut pcm: Vec<i16> = Vec::new();
    let res = loop {
        if !shared.want.load(Ordering::Relaxed) {
            break Ok(());
        }
        // the default output changed: follow it
        if checked.elapsed() > Duration::from_secs(2) {
            checked = Instant::now();
            if en.GetDefaultAudioEndpoint(eRender, eConsole).map(|d| device_id(&d)).unwrap_or_default() != id {
                break Ok(());
            }
        }
        std::thread::sleep(Duration::from_millis(10));
        pcm.clear();
        loop {
            let n = match capture.GetNextPacketSize() {
                Ok(n) => n,
                Err(e) => {
                    let _ = client.Stop();
                    return Err(e);
                }
            };
            if n == 0 {
                break;
            }
            let (mut data, mut frames, mut flags) = (std::ptr::null_mut(), 0u32, 0u32);
            capture.GetBuffer(&mut data, &mut frames, &mut flags, None, None)?;
            let silent = flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0;
            conv.push(data, frames as usize, silent, &mut pcm);
            capture.ReleaseBuffer(frames)?;
        }
        if pcm.is_empty() || shared.paused.load(Ordering::Relaxed) {
            continue;
        }
        let now = shared.clock.lock().unwrap().now();
        shared.audio.lock().unwrap().push(&pcm, now);
    };
    let _ = client.Stop();
    res
}

/// Any layout at any rate into stereo at 48 kHz.
struct Convert {
    channels: usize,
    kind: Sample,
    left: crate::voice::Resampler,
    right: crate::voice::Resampler,
    l: Vec<f32>,
    r: Vec<f32>,
    out_l: Vec<f32>,
    out_r: Vec<f32>,
}

impl Convert {
    fn new(channels: usize, rate: u32, kind: Sample) -> Self {
        Self {
            channels,
            kind,
            left: crate::voice::Resampler::new(rate, super::RATE as u32),
            right: crate::voice::Resampler::new(rate, super::RATE as u32),
            l: Vec::new(),
            r: Vec::new(),
            out_l: Vec::new(),
            out_r: Vec::new(),
        }
    }

    unsafe fn push(&mut self, data: *const u8, frames: usize, silent: bool, out: &mut Vec<i16>) {
        let ch = self.channels.max(1);
        self.l.clear();
        self.r.clear();
        if silent {
            self.l.resize(frames, 0.0);
            self.r.resize(frames, 0.0);
        } else {
            let read = |i: usize| -> f32 {
                match self.kind {
                    Sample::F32 => (data as *const f32).add(i).read_unaligned(),
                    Sample::I16 => (data as *const i16).add(i).read_unaligned() as f32 / 32768.0,
                    Sample::I32 => (data as *const i32).add(i).read_unaligned() as f32 / 2_147_483_648.0,
                }
            };
            let mut frame = [0f32; 8];
            for f in 0..frames {
                for (c, v) in frame.iter_mut().enumerate().take(ch.min(8)) {
                    *v = read(f * ch + c);
                }
                let (a, b) = downmix(&frame[..ch.min(8)]);
                self.l.push(a);
                self.r.push(b);
            }
        }
        self.out_l.clear();
        self.out_r.clear();
        self.left.push(&self.l, &mut self.out_l);
        self.right.push(&self.r, &mut self.out_r);
        for (a, b) in self.out_l.iter().zip(self.out_r.iter()) {
            out.push((a.clamp(-1.0, 1.0) * 32767.0) as i16);
            out.push((b.clamp(-1.0, 1.0) * 32767.0) as i16);
        }
    }
}

/// One frame of any layout as left and right. The surround order is Windows': FL FR FC LFE BL BR SL SR.
fn downmix(f: &[f32]) -> (f32, f32) {
    match f.len() {
        0 => (0.0, 0.0),
        1 => (f[0], f[0]),
        2..=5 => (f[0], f[1]),
        n => {
            const SIDE: f32 = 0.707;
            let mut l = f[0] + f[2] * SIDE + f[4] * SIDE;
            let mut r = f[1] + f[2] * SIDE + f[5] * SIDE;
            if n >= 8 {
                l += f[6] * SIDE;
                r += f[7] * SIDE;
            }
            let k = if n >= 8 { 0.32 } else { 0.41 };
            (l * k, r * k)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layouts() {
        assert_eq!(downmix(&[0.5]), (0.5, 0.5));
        assert_eq!(downmix(&[0.1, 0.2]), (0.1, 0.2));
        // the centre channel (voices) reaches both sides, and full level doesn't clip
        let (l, r) = downmix(&[0.0, 0.0, 1.0, 0.0, 0.0, 0.0]);
        assert!(l > 0.2 && (l - r).abs() < 1e-6);
        let (l, _) = downmix(&[1.0; 8]);
        assert!(l <= 1.0, "{l}");
        let (l, _) = downmix(&[1.0; 6]);
        assert!(l <= 1.0, "{l}");
    }

    #[test]
    fn a_silent_packet_is_silence_at_48_khz() {
        let mut c = Convert::new(2, 48_000, Sample::F32);
        let mut out = Vec::new();
        unsafe { c.push(std::ptr::null(), 480, true, &mut out) };
        assert_eq!(out.len(), 960);
        assert!(out.iter().all(|v| *v == 0));
    }

    #[test]
    fn samples_become_16_bit_stereo() {
        let mut c = Convert::new(2, 48_000, Sample::F32);
        let data: [f32; 4] = [0.5, -0.5, 1.5, -1.5];
        let mut out = Vec::new();
        unsafe { c.push(data.as_ptr() as *const u8, 2, false, &mut out) };
        assert_eq!(out, vec![16383, -16383, 32767, -32767]);
    }
}

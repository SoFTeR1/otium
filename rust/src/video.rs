//! Silent looping video for the background, decoded by Windows itself (Media Foundation), so no
//! codec ships with OTIUM. A worker thread decodes at the video's own pace and leaves the latest
//! frame for the UI; it stops decoding while paused and is gone once the player is dropped.

use egui::{Color32, ColorImage};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use windows::core::{GUID, PCWSTR};
use windows::Win32::Media::MediaFoundation::*;

/// Larger videos are scaled down while decoding: a background doesn't need more, and memory and
/// upload time grow with every pixel.
const MAX_WIDTH: u32 = 1280;
const ALL_STREAMS: u32 = 0xFFFF_FFFE;
const FIRST_VIDEO: u32 = 0xFFFF_FFFC;
const END_OF_STREAM: u32 = 2; // MF_SOURCE_READERF_ENDOFSTREAM

#[derive(Default)]
struct Shared {
    frame: Mutex<Option<ColorImage>>,
    playing: AtomicBool,
    stop: AtomicBool,
    failed: AtomicBool,
    /// share of the height taken by the black bar at the top and at the bottom, in thousandths
    bars: AtomicU32,
    max_width: AtomicU32,
    /// played once and stays on its last frame (a clip being watched), instead of looping
    once: AtomicBool,
    ended: AtomicBool,
}

pub struct Player {
    shared: Arc<Shared>,
    path: PathBuf,
}

impl Player {
    /// `wake` is called for every new frame.
    pub fn start(path: &Path, wake: impl Fn() + Send + 'static) -> Self {
        Self::spawn(path, MAX_WIDTH, false, wake)
    }

    /// A clip being watched: full size, once through.
    pub fn start_once(path: &Path, wake: impl Fn() + Send + 'static) -> Self {
        Self::spawn(path, 1920, true, wake)
    }

    fn spawn(path: &Path, max_width: u32, once: bool, wake: impl Fn() + Send + 'static) -> Self {
        let shared = Arc::new(Shared::default());
        shared.playing.store(true, Ordering::Relaxed);
        shared.max_width.store(max_width, Ordering::Relaxed);
        shared.once.store(once, Ordering::Relaxed);
        let (s, p) = (shared.clone(), path.to_path_buf());
        let _ = std::thread::Builder::new().name("video".into()).spawn(move || run(&p, &s, wake));
        Self { shared, path: path.to_path_buf() }
    }

    /// Played to the end (only a clip being watched gets there).
    pub fn ended(&self) -> bool {
        self.shared.ended.load(Ordering::Relaxed)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// The newest frame since the last call.
    pub fn take(&self) -> Option<ColorImage> {
        self.shared.frame.lock().unwrap().take()
    }

    pub fn set_playing(&self, on: bool) {
        self.shared.playing.store(on, Ordering::Relaxed);
    }

    /// How much of the picture's height is a black bar, at each of top and bottom (0.0 ..): film
    /// trailers have them, often with subtitles inside.
    pub fn bars(&self) -> f32 {
        self.shared.bars.load(Ordering::Relaxed) as f32 / 1000.0
    }

    /// The file can't be played (nothing to show: the picture under it stays).
    pub fn failed(&self) -> bool {
        self.shared.failed.load(Ordering::Relaxed)
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::Relaxed);
    }
}

fn run(path: &Path, s: &Shared, wake: impl Fn()) {
    unsafe {
        let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
        if MFStartup(MF_VERSION, MFSTARTUP_LITE).is_err() {
            s.failed.store(true, Ordering::Relaxed);
            return;
        }
    }
    let once = s.once.load(Ordering::Relaxed);
    if !once {
        if let Ok(b) = unsafe { scan_bars(path) } {
            s.bars.store((b * 1000.0) as u32, Ordering::Relaxed);
        }
    }
    // played through at least once, so a failure on reopening doesn't count as "can't be played"
    let mut played = false;
    while !s.stop.load(Ordering::Relaxed) {
        match unsafe { play_once(path, s, &wake) } {
            Ok(frames) if frames > 0 => played = true,
            Ok(_) | Err(_) => {
                if !played {
                    s.failed.store(true, Ordering::Relaxed);
                }
                break;
            }
        }
        if once {
            s.ended.store(true, Ordering::Relaxed);
            wake();
            break;
        }
    }
    unsafe {
        let _ = MFShutdown();
    }
}

unsafe fn open(path: &Path, max_width: u32) -> windows::core::Result<IMFSourceReader> {
    let mut attrs = None;
    MFCreateAttributes(&mut attrs, 1)?;
    let attrs: IMFAttributes = attrs.ok_or(windows::core::Error::empty())?;
    // converts to RGB and resizes in the decoding pipeline
    attrs.SetUINT32(&MF_SOURCE_READER_ENABLE_ADVANCED_VIDEO_PROCESSING, 1)?;
    let url: Vec<u16> = path.as_os_str().to_string_lossy().encode_utf16().chain(Some(0)).collect();
    let reader = MFCreateSourceReaderFromURL(PCWSTR(url.as_ptr()), &attrs)?;
    // the picture only: audio isn't even decoded
    reader.SetStreamSelection(ALL_STREAMS, false)?;
    reader.SetStreamSelection(FIRST_VIDEO, true)?;

    let want = |subtype: &GUID, size: Option<u64>| -> windows::core::Result<IMFMediaType> {
        let t = MFCreateMediaType()?;
        t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)?;
        t.SetGUID(&MF_MT_SUBTYPE, subtype)?;
        if let Some(sz) = size {
            t.SetUINT64(&MF_MT_FRAME_SIZE, sz)?;
        }
        Ok(t)
    };
    let native = reader.GetNativeMediaType(FIRST_VIDEO, 0)?.GetUINT64(&MF_MT_FRAME_SIZE).unwrap_or(0);
    let (w, h) = ((native >> 32) as u32, native as u32);
    let scaled = (w > max_width && h > 0).then(|| {
        // even sizes: what scalers expect
        let nh = (h as u64 * max_width as u64 / w as u64) as u32 & !1;
        ((max_width as u64) << 32) | nh as u64
    });
    let t = want(&MFVideoFormat_RGB32, scaled)?;
    if reader.SetCurrentMediaType(FIRST_VIDEO, None, &t).is_err() {
        reader.SetCurrentMediaType(FIRST_VIDEO, None, &want(&MFVideoFormat_RGB32, None)?)?;
    }
    Ok(reader)
}

/// One pass through the file. Returns how many frames it had.
unsafe fn play_once(path: &Path, s: &Shared, wake: &impl Fn()) -> windows::core::Result<u32> {
    let reader = open(path, s.max_width.load(Ordering::Relaxed))?;
    let cur = reader.GetCurrentMediaType(FIRST_VIDEO)?;
    let sz = cur.GetUINT64(&MF_MT_FRAME_SIZE)?;
    let (w, h) = ((sz >> 32) as u32, sz as u32);
    let stride = match cur.GetUINT32(&MF_MT_DEFAULT_STRIDE) {
        Ok(v) => v as i32,
        Err(_) => (w * 4) as i32, // not stated: this pipeline delivers rows top-down
    };
    let mut frames = 0;
    let mut clock = Instant::now();
    let mut paused_for = Duration::ZERO;
    // a background doesn't need more than 30 pictures a second (some trailers are 60)
    let mut shown: Option<Duration> = None;
    loop {
        // pausing keeps the place in the video and decodes nothing
        while !s.playing.load(Ordering::Relaxed) {
            if s.stop.load(Ordering::Relaxed) {
                return Ok(frames);
            }
            std::thread::sleep(Duration::from_millis(60));
            paused_for += Duration::from_millis(60);
        }
        if s.stop.load(Ordering::Relaxed) {
            return Ok(frames);
        }
        clock += std::mem::take(&mut paused_for);
        let (mut flags, mut ts, mut sample) = (0u32, 0i64, None);
        reader.ReadSample(FIRST_VIDEO, 0, None, Some(&mut flags), Some(&mut ts), Some(&mut sample))?;
        if flags & END_OF_STREAM != 0 {
            return Ok(frames);
        }
        let Some(sample) = sample else { continue };
        let at = Duration::from_nanos(ts.max(0) as u64 * 100);
        if shown.is_some_and(|s| at < s + Duration::from_millis(30)) {
            frames += 1;
            continue;
        }
        shown = Some(at);
        let now = clock.elapsed();
        if at > now {
            std::thread::sleep((at - now).min(Duration::from_millis(500)));
        } else if now - at > Duration::from_millis(150) && frames > 0 {
            // running late (the machine is busy): skip the picture, not the time
            frames += 1;
            continue;
        }
        let image = to_image(&sample, w, h, stride)?;
        *s.frame.lock().unwrap() = Some(image);
        frames += 1;
        wake();
    }
}

/// Looks through the first seconds for the black bars at the top and bottom. Decoding runs far faster
/// than playback, so this takes a fraction of a second.
unsafe fn scan_bars(path: &Path) -> windows::core::Result<f32> {
    const SECONDS: i64 = 12;
    let reader = open(path, MAX_WIDTH)?;
    let cur = reader.GetCurrentMediaType(FIRST_VIDEO)?;
    let sz = cur.GetUINT64(&MF_MT_FRAME_SIZE)?;
    let (w, h) = ((sz >> 32) as u32, sz as u32);
    let stride = cur.GetUINT32(&MF_MT_DEFAULT_STRIDE).map_or((w * 4) as i32, |v| v as i32);
    let mut best: Option<f32> = None;
    let mut next = 0i64;
    loop {
        let (mut flags, mut ts, mut sample) = (0u32, 0i64, None);
        reader.ReadSample(FIRST_VIDEO, 0, None, Some(&mut flags), Some(&mut ts), Some(&mut sample))?;
        if flags & END_OF_STREAM != 0 || ts > SECONDS * 10_000_000 {
            break;
        }
        let Some(sample) = sample else { continue };
        if ts < next {
            continue;
        }
        next = ts + 5_000_000; // two a second
        if let Some(b) = frame_bars(&to_image(&sample, w, h, stride)?) {
            best = Some(best.map_or(b, |x: f32| x.min(b)));
        }
    }
    Ok(best.unwrap_or(0.0))
}

fn luma(c: Color32) -> u32 {
    (c.r() as u32 * 2 + c.g() as u32 * 5 + c.b() as u32) / 8
}

/// The bars of one picture, or None when it is too dark to tell a bar from the scene. A row counts
/// as part of a bar when little of it is lit, so subtitles inside a bar don't end it.
fn frame_bars(img: &ColorImage) -> Option<f32> {
    let [w, h] = img.size;
    if w == 0 || h == 0 {
        return None;
    }
    let row = |y: usize| &img.pixels[y * w..(y + 1) * w];
    let lit = |y: usize| row(y).iter().step_by(4).filter(|c| luma(**c) > 40).count() * 4 > w / 4;
    let middle: u64 = (h * 2 / 5..h * 3 / 5).map(|y| row(y).iter().step_by(8).map(|c| luma(*c) as u64).sum::<u64>()).sum();
    let count = (h / 5).max(1) as u64 * (w as u64 / 8).max(1);
    if middle / count < 25 {
        return None;
    }
    let top = (0..h / 3).find(|y| lit(*y)).unwrap_or(h / 3);
    let bottom = (0..h / 3).find(|y| lit(h - 1 - *y)).unwrap_or(h / 3);
    Some(top.min(bottom) as f32 / h as f32)
}

unsafe fn to_image(sample: &IMFSample, w: u32, h: u32, stride: i32) -> windows::core::Result<ColorImage> {
    let buf = sample.ConvertToContiguousBuffer()?;
    let (mut p, mut len) = (std::ptr::null_mut(), 0u32);
    buf.Lock(&mut p, None, Some(&mut len))?;
    let data = std::slice::from_raw_parts(p, len as usize);
    let img = bgrx_to_image(data, w as usize, h as usize, stride);
    let _ = buf.Unlock();
    Ok(img)
}

/// BGRX rows (bottom-up when the stride is negative) into an opaque image.
fn bgrx_to_image(data: &[u8], w: usize, h: usize, stride: i32) -> ColorImage {
    let pitch = stride.unsigned_abs() as usize;
    let mut pixels = Vec::with_capacity(w * h);
    for y in 0..h {
        let row = if stride < 0 { h - 1 - y } else { y } * pitch;
        match data.get(row..row + w * 4) {
            Some(r) => pixels.extend(r.chunks_exact(4).map(|c| Color32::from_rgb(c[2], c[1], c[0]))),
            None => pixels.extend(std::iter::repeat_n(Color32::BLACK, w)),
        }
    }
    ColorImage { size: [w, h], pixels }
}

/// A clip's still: a picture from its middle, as a JPEG at most `width` wide. Works on what friends
/// sent too, so it only trusts what Windows reads from the file.
pub fn poster(path: &Path, out: &Path, width: u32) -> bool {
    unsafe {
        let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
        if MFStartup(MF_VERSION, MFSTARTUP_LITE).is_err() {
            return false;
        }
        let img = middle_frame(path);
        let _ = MFShutdown();
        let Ok(img) = img else { return false };
        let rgb: Vec<u8> = img.pixels.iter().flat_map(|c| [c.r(), c.g(), c.b()]).collect();
        let Some(buf) = image::RgbImage::from_raw(img.size[0] as u32, img.size[1] as u32, rgb) else { return false };
        let small = image::DynamicImage::ImageRgb8(buf).thumbnail(width, width);
        if let Some(dir) = out.parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        // written aside and renamed, so a half-written file is never painted (and cached as broken)
        let tmp = out.with_extension("tmp.jpg");
        small.save_with_format(&tmp, image::ImageFormat::Jpeg).is_ok() && std::fs::rename(&tmp, out).is_ok()
    }
}

/// How long a file plays, in 100 ns units (0 if it doesn't say).
unsafe fn duration_of(reader: &IMFSourceReader) -> i64 {
    const MEDIA_SOURCE: u32 = 0xFFFF_FFFF; // MF_SOURCE_READER_MEDIASOURCE
    reader.GetPresentationAttribute(MEDIA_SOURCE, &MF_PD_DURATION).ok().and_then(|v| i64::try_from(&v).ok().or_else(|| u64::try_from(&v).ok().map(|x| x as i64))).unwrap_or(0)
}

unsafe fn middle_frame(path: &Path) -> windows::core::Result<ColorImage> {
    let reader = open(path, MAX_WIDTH)?;
    let middle = duration_of(&reader) / 2;
    if middle > 0 {
        // lands on the key frame before it; what follows is read up to the middle
        let _ = reader.SetCurrentPosition(&GUID::zeroed(), &windows::core::PROPVARIANT::from(middle));
    }
    let cur = reader.GetCurrentMediaType(FIRST_VIDEO)?;
    let sz = cur.GetUINT64(&MF_MT_FRAME_SIZE)?;
    let (w, h) = ((sz >> 32) as u32, sz as u32);
    let stride = cur.GetUINT32(&MF_MT_DEFAULT_STRIDE).map_or((w * 4) as i32, |v| v as i32);
    let mut last = None;
    loop {
        let (mut flags, mut ts, mut sample) = (0u32, 0i64, None);
        reader.ReadSample(FIRST_VIDEO, 0, None, Some(&mut flags), Some(&mut ts), Some(&mut sample))?;
        if let Some(s) = sample {
            if ts >= middle {
                return to_image(&s, w, h, stride);
            }
            last = Some(s);
        }
        if flags & END_OF_STREAM != 0 {
            return match last {
                Some(s) => to_image(&s, w, h, stride),
                None => Err(windows::core::Error::empty()),
            };
        }
    }
}

/// A file's sound, decoded by Windows: stereo 16-bit at 48 kHz (empty if it has none).
pub fn sound_of(path: &Path) -> Vec<i16> {
    unsafe {
        let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
        if MFStartup(MF_VERSION, MFSTARTUP_LITE).is_err() {
            return Vec::new();
        }
        let out = read_sound(path).unwrap_or_default();
        let _ = MFShutdown();
        out
    }
}

unsafe fn read_sound(path: &Path) -> windows::core::Result<Vec<i16>> {
    const FIRST_AUDIO: u32 = 0xFFFF_FFFD;
    let url: Vec<u16> = path.as_os_str().to_string_lossy().encode_utf16().chain(Some(0)).collect();
    let reader = MFCreateSourceReaderFromURL(PCWSTR(url.as_ptr()), None)?;
    reader.SetStreamSelection(ALL_STREAMS, false)?;
    reader.SetStreamSelection(FIRST_AUDIO, true)?;
    let t = MFCreateMediaType()?;
    t.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)?;
    t.SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_PCM)?;
    t.SetUINT32(&MF_MT_AUDIO_BITS_PER_SAMPLE, 16)?;
    t.SetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND, 48_000)?;
    t.SetUINT32(&MF_MT_AUDIO_NUM_CHANNELS, 2)?;
    t.SetUINT32(&MF_MT_AUDIO_BLOCK_ALIGNMENT, 4)?;
    t.SetUINT32(&MF_MT_AUDIO_AVG_BYTES_PER_SECOND, 192_000)?;
    reader.SetCurrentMediaType(FIRST_AUDIO, None, &t)?;
    let mut out = Vec::new();
    loop {
        let (mut flags, mut sample) = (0u32, None);
        reader.ReadSample(FIRST_AUDIO, 0, None, Some(&mut flags), None, Some(&mut sample))?;
        if let Some(s) = sample {
            let buf = s.ConvertToContiguousBuffer()?;
            let (mut p, mut len) = (std::ptr::null_mut(), 0u32);
            buf.Lock(&mut p, None, Some(&mut len))?;
            out.extend(std::slice::from_raw_parts(p, len as usize).chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])));
            let _ = buf.Unlock();
        }
        if flags & END_OF_STREAM != 0 {
            return Ok(out);
        }
    }
}

/// What a test wants to know about a file.
#[cfg(test)]
#[derive(Debug)]
pub struct Probe {
    pub frames: u32,
    pub width: u32,
    pub height: u32,
    pub seconds: f64,
    pub audio_samples: usize,
}

#[cfg(test)]
pub fn probe(path: &Path) -> windows::core::Result<Probe> {
    unsafe {
        let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
        MFStartup(MF_VERSION, MFSTARTUP_LITE)?;
        let reader = open(path, 4096)?;
        let cur = reader.GetCurrentMediaType(FIRST_VIDEO)?;
        let sz = cur.GetUINT64(&MF_MT_FRAME_SIZE)?;
        let seconds = duration_of(&reader) as f64 / 1e7;
        let mut frames = 0;
        loop {
            let (mut flags, mut sample) = (0u32, None);
            reader.ReadSample(FIRST_VIDEO, 0, None, Some(&mut flags), None, Some(&mut sample))?;
            frames += sample.is_some() as u32;
            if flags & END_OF_STREAM != 0 {
                break;
            }
        }
        let audio_samples = read_sound(path)?.len() / 2;
        let _ = MFShutdown();
        Ok(Probe { frames, width: (sz >> 32) as u32, height: sz as u32, seconds, audio_samples })
    }
}

/// What a file's name says it is.
pub fn is_video(path: &str) -> bool {
    let p = path.split(['?', '#']).next().unwrap_or("").to_ascii_lowercase();
    [".mp4", ".webm", ".mkv", ".mov", ".m4v", ".avi", ".wmv"].iter().any(|e| p.ends_with(e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_in_order() {
        // two rows of one pixel: blue, then red
        let data = [255, 0, 0, 0, 0, 0, 255, 0];
        let down = bgrx_to_image(&data, 1, 2, 4);
        assert_eq!(down.pixels, vec![Color32::from_rgb(0, 0, 255), Color32::from_rgb(255, 0, 0)]);
        let up = bgrx_to_image(&data, 1, 2, -4);
        assert_eq!(up.pixels, vec![Color32::from_rgb(255, 0, 0), Color32::from_rgb(0, 0, 255)]);
    }

    fn picture(rows: &[(usize, u8, usize)]) -> ColorImage {
        // (rows, brightness, how many of 100 pixels are lit)
        let mut pixels = Vec::new();
        for (n, v, lit) in rows {
            for _ in 0..*n {
                pixels.extend((0..100).map(|x| if x < *lit { Color32::from_gray(*v) } else { Color32::BLACK }));
            }
        }
        ColorImage { size: [100, pixels.len() / 100], pixels }
    }

    #[test]
    fn finds_film_bars() {
        let img = picture(&[(10, 0, 0), (80, 120, 100), (10, 0, 0)]);
        assert_eq!(frame_bars(&img), Some(0.1));
    }

    #[test]
    fn subtitles_stay_inside_the_bar() {
        let img = picture(&[(10, 0, 0), (80, 120, 100), (3, 0, 0), (4, 255, 15), (3, 0, 0)]);
        assert_eq!(frame_bars(&img), Some(0.1));
    }

    #[test]
    fn a_full_picture_has_none_and_a_dark_one_says_nothing() {
        assert_eq!(frame_bars(&picture(&[(100, 120, 100)])), Some(0.0));
        assert_eq!(frame_bars(&picture(&[(100, 10, 100)])), None);
    }

    #[test]
    fn short_buffer_is_not_a_crash() {
        let img = bgrx_to_image(&[0; 4], 1, 2, 4);
        assert_eq!(img.pixels.len(), 2);
        assert_eq!(img.pixels[1], Color32::BLACK);
    }

    #[test]
    fn video_names() {
        assert!(is_video("file:///D:/x/Фон.MP4"));
        assert!(is_video("https://x/y.webm?t=1"));
        assert!(!is_video("file:///D:/x/a.png"));
        assert!(!is_video(""));
    }
}

#[cfg(test)]
mod live {
    use super::*;

    /// cargo test --release live_trailer -- --ignored --nocapture
    /// Downloads a real trailer, decodes it for a few seconds and saves a frame next to it.
    #[test]
    #[ignore]
    fn live_trailer() {
        let t = Instant::now();
        let file = crate::trailer::fetch("1245620").expect("no trailer");
        let mb = std::fs::metadata(&file).unwrap().len() as f64 / 1e6;
        println!("{file:?}: {mb:.1} MB in {:.1?}", t.elapsed());
        let n = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let c = n.clone();
        let p = Player::start(&file, move || {
            c.fetch_add(1, Ordering::Relaxed);
        });
        std::thread::sleep(Duration::from_secs(6));
        let frame = p.take().expect("no frame");
        let count = n.load(Ordering::Relaxed);
        println!("{} frames in 6 s, {}x{}, failed: {}", count, frame.size[0], frame.size[1], p.failed());
        let rgb: Vec<u8> = frame.pixels.iter().flat_map(|c| [c.r(), c.g(), c.b()]).collect();
        let out = file.with_extension("png");
        image::save_buffer(&out, &rgb, frame.size[0] as u32, frame.size[1] as u32, image::ColorType::Rgb8).unwrap();
        println!("saved {out:?}");
        assert!(count > 100, "too slow: {count}");
    }
}

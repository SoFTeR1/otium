//! Clips and screenshots, like Create on a PS5: while a game has the screen the last 30 seconds of
//! picture and sound stay in memory, and one press writes them to an MP4.
//!
//! Everything comes with Windows, so no codec ships with OTIUM: the screen through DXGI Desktop
//! Duplication, H.264 through whatever Media Foundation encoder the GPU brings (or Microsoft's
//! software one), sound through WASAPI loopback. Nothing touches the disk until a clip is saved.
//!
//! Recording stops while OTIUM itself is in front, and so does the clock the clip is timed by: the
//! menus never end up in a clip, and the last game frame is still there for a screenshot taken from
//! the Control Center.

mod capture;
mod encoder;
mod loopback;
mod mux;

use encoder::Packet;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How much a clip holds.
pub const CLIP_SECONDS: i64 = 30;
const SEC: i64 = 10_000_000;
pub(crate) const RATE: i64 = 48_000;

/// What finished in the background: where the file is, how long a clip is (0 for a picture).
#[derive(Debug, Clone)]
pub struct Saved {
    pub path: PathBuf,
    pub secs: u32,
}

type Done = Box<dyn FnOnce(Result<Saved, String>) + Send>;

pub(crate) struct ShotRequest {
    path: PathBuf,
    done: Done,
}

/// Runs while the game has the screen. Picture and sound are both timed by it, in 100 ns units,
/// so a pause in front of OTIUM leaves no hole in the clip.
pub(crate) struct Clock {
    base: Instant,
    paused: Duration,
    since: Option<Instant>,
}

impl Clock {
    fn new() -> Self {
        Self { base: Instant::now(), paused: Duration::ZERO, since: None }
    }

    fn now(&self) -> i64 {
        let at = self.since.unwrap_or_else(Instant::now);
        ((at - self.base).saturating_sub(self.paused).as_nanos() / 100) as i64
    }

    fn set_paused(&mut self, on: bool) {
        match (on, self.since) {
            (true, None) => self.since = Some(Instant::now()),
            (false, Some(t)) => {
                self.paused += t.elapsed();
                self.since = None;
            }
            _ => {}
        }
    }
}

/// The encoded picture of the last 30 seconds, always starting on a key frame.
#[derive(Default)]
pub(crate) struct VideoRing {
    pub packets: VecDeque<Arc<Packet>>,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate: u32,
    /// SPS and PPS, when the encoder states them outside the stream
    pub header: Option<Vec<u8>>,
}

impl VideoRing {
    pub fn push(&mut self, p: Packet) {
        let cut = p.ts - CLIP_SECONDS * SEC;
        self.packets.push_back(Arc::new(p));
        // the last key frame old enough to start a clip from stays, what's before it goes
        if let Some(i) = self.packets.iter().rposition(|p| p.key && p.ts <= cut) {
            self.packets.drain(..i);
        }
    }

    /// The packets of a clip ending at `end`.
    fn clip(&self, end: i64) -> Vec<Arc<Packet>> {
        let cut = end - CLIP_SECONDS * SEC;
        let list: Vec<&Arc<Packet>> = self.packets.iter().filter(|p| p.ts <= end).collect();
        let start = list.iter().rposition(|p| p.key && p.ts <= cut).or_else(|| list.iter().position(|p| p.key));
        start.map_or(Vec::new(), |i| list[i..].iter().map(|p| (*p).clone()).collect())
    }
}

/// The sound of the last 30 seconds, stereo at 48 kHz, without holes: what played nothing is silence.
#[derive(Default)]
pub(crate) struct AudioRing {
    samples: VecDeque<i16>,
    /// clock time of the end of `samples`
    end: i64,
}

impl AudioRing {
    fn frames_of(t: i64) -> i64 {
        t * RATE / SEC
    }

    /// `pcm` has just arrived, at clock time `now`.
    pub fn push(&mut self, pcm: &[i16], now: i64) {
        let frames = pcm.len() as i64 / 2;
        let dur = frames * SEC / RATE;
        let gap = now - dur - self.end;
        if self.samples.is_empty() {
            self.end = now - dur;
        } else if gap > SEC / 25 {
            // the loopback delivers nothing while nothing plays
            let fill = Self::frames_of(gap.min((CLIP_SECONDS + 2) * SEC)) as usize * 2;
            self.samples.extend(std::iter::repeat_n(0, fill));
            self.end += fill as i64 / 2 * SEC / RATE;
            if gap > (CLIP_SECONDS + 2) * SEC {
                self.end = now - dur;
            }
        } else if gap < -SEC / 10 {
            // the sound card's clock ran ahead of ours
            self.end = now - dur;
        }
        self.samples.extend(pcm.iter().copied());
        self.end += dur;
        let max = ((CLIP_SECONDS + 4) * RATE) as usize * 2;
        if self.samples.len() > max {
            let extra = (self.samples.len() - max) & !1;
            self.samples.drain(..extra);
        }
    }

    /// Stereo samples of the stretch from `t0` to `t1`; anything not there is silence.
    pub fn cut(&self, t0: i64, t1: i64) -> Vec<i16> {
        let frames = Self::frames_of((t1 - t0).max(0)) as usize;
        let mut out = vec![0i16; frames * 2];
        let start = self.end - (self.samples.len() as i64 / 2) * SEC / RATE;
        let from = Self::frames_of(t0 - start);
        for (i, o) in out.chunks_exact_mut(2).enumerate() {
            let at = from + i as i64;
            if at >= 0 {
                let at = at as usize * 2;
                if at + 1 < self.samples.len() {
                    o[0] = self.samples[at];
                    o[1] = self.samples[at + 1];
                }
            }
        }
        out
    }

    pub fn clear(&mut self) {
        self.samples.clear();
    }
}

pub(crate) struct Shared {
    pub want: AtomicBool,
    pub own: Arc<AtomicIsize>,
    pub clock: Mutex<Clock>,
    pub video: Mutex<VideoRing>,
    pub audio: Mutex<AudioRing>,
    pub shots: Mutex<Vec<ShotRequest>>,
    /// OTIUM is in front: nothing is being recorded
    pub paused: AtomicBool,
}

pub struct Recorder {
    shared: Arc<Shared>,
}

impl Recorder {
    /// `own` is OTIUM's window: while it is in front nothing is recorded.
    pub fn start(own: Arc<AtomicIsize>) -> Self {
        let shared = Arc::new(Shared {
            want: AtomicBool::new(false),
            own,
            clock: Mutex::new(Clock::new()),
            video: Default::default(),
            audio: Default::default(),
            shots: Default::default(),
            paused: AtomicBool::new(false),
        });
        capture::start(shared.clone());
        loopback::start(shared.clone());
        Self { shared }
    }

    /// Keep the last 30 seconds (a game is on) or not. Turned off, what was kept is forgotten.
    pub fn set_wanted(&self, on: bool) {
        self.shared.want.store(on, Ordering::Relaxed);
    }

    pub fn wanted(&self) -> bool {
        self.shared.want.load(Ordering::Relaxed)
    }

    /// Writes the last 30 seconds in the background.
    pub fn save_clip(&self, title: &str, done: impl FnOnce(Result<Saved, String>) + Send + 'static) {
        let shared = self.shared.clone();
        let path = new_path(title, "mp4");
        std::thread::Builder::new()
            .name("clip-save".into())
            .spawn(move || done(save(&shared, path)))
            .ok();
    }

    /// Takes a picture of the screen, or of the game while OTIUM is over it.
    pub fn screenshot(&self, title: &str, done: impl FnOnce(Result<Saved, String>) + Send + 'static) {
        let path = new_path(title, "png");
        self.shared.shots.lock().unwrap().push(ShotRequest { path, done: Box::new(done) });
    }
}

fn save(shared: &Shared, path: PathBuf) -> Result<Saved, String> {
    let end = shared.clock.lock().unwrap().now();
    let (packets, width, height, fps, bitrate, header) = {
        let v = shared.video.lock().unwrap();
        (v.clip(end), v.width, v.height, v.fps, v.bitrate, v.header.clone())
    };
    const EMPTY: &str = "Клип пока пуст: запись идёт, только пока на экране игра";
    let t0 = packets.first().map(|p| p.ts).ok_or(EMPTY)?;
    let t1 = packets.last().map(|p| p.ts).unwrap_or(t0);
    if t1 - t0 < SEC {
        return Err(EMPTY.into());
    }
    let pcm = shared.audio.lock().unwrap().cut(t0, t1);
    let dir = path.parent().map(PathBuf::from).unwrap_or_default();
    std::fs::create_dir_all(&dir).map_err(|e| format!("Не удалось создать папку клипов: {e}"))?;
    let part = path.with_extension("part.mp4");
    let info = mux::Info { width, height, fps, bitrate, header };
    let res = unsafe { mux::write(&part, &info, &packets, &pcm) };
    if let Err(e) = res {
        let _ = std::fs::remove_file(&part);
        eprintln!("clip: {e}");
        return Err("Не удалось сохранить клип".into());
    }
    std::fs::rename(&part, &path).map_err(|e| format!("Не удалось сохранить клип: {e}"))?;
    Ok(Saved { path, secs: ((t1 - t0) / SEC) as u32 })
}

static CHOSEN: Mutex<Option<PathBuf>> = Mutex::new(None);

/// The folder picked in the settings (None: the default one).
pub fn set_folder(dir: Option<PathBuf>) {
    *CHOSEN.lock().unwrap() = dir;
}

/// Where clips and screenshots live: what was picked, or Videos\OTIUM.
pub fn folder() -> PathBuf {
    if let Some(d) = std::env::var_os("OTIUM_CLIPS") {
        return PathBuf::from(d);
    }
    if let Some(d) = CHOSEN.lock().unwrap().clone() {
        return d;
    }
    dirs::video_dir().unwrap_or_else(crate::data::Library::data_dir).join("OTIUM")
}

/// What friends sent: a folder inside.
pub fn received_folder() -> PathBuf {
    folder().join("От друзей")
}

/// A file name Windows accepts: what it forbids is dropped, the rest stays readable.
pub fn safe_name(title: &str) -> String {
    let clean: String = title.chars().filter(|c| !c.is_control() && !"<>:\"/\\|?*".contains(*c)).take(60).collect();
    let clean = clean.trim().trim_matches('.').trim();
    if clean.is_empty() { "Клип".into() } else { clean.into() }
}

/// The folders inside `dir` to offer in the chooser, by name (hidden and system ones aren't).
pub fn subfolders(dir: &std::path::Path) -> Vec<PathBuf> {
    use std::os::windows::fs::MetadataExt;
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<PathBuf> = rd.flatten().filter(|e| e.metadata().is_ok_and(|m| m.is_dir() && m.file_attributes() & 0x6 == 0)).map(|e| e.path()).collect();
    out.sort_by_key(|p| p.file_name().map(|n| n.to_string_lossy().to_lowercase()));
    out
}

/// Whether Windows takes this as a folder name.
pub fn folder_name_ok(name: &str) -> bool {
    let n = name.trim();
    let base = n.split('.').next().unwrap_or(n).to_ascii_uppercase();
    let reserved = matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL") || (base.len() == 4 && (base.starts_with("COM") || base.starts_with("LPT")) && base.ends_with(|c: char| c.is_ascii_digit()));
    !n.is_empty() && !reserved && !n.ends_with('.') && !n.chars().any(|c| c.is_control() || r#"\/:*?"<>|"#.contains(c))
}

fn stamped(dir: PathBuf, title: &str, ext: &str) -> PathBuf {
    let stamp = chrono::Local::now().format("%Y-%m-%d %H-%M-%S");
    let name = safe_name(title);
    let mut path = dir.join(format!("{name} {stamp}.{ext}"));
    let mut n = 2;
    while path.exists() {
        path = dir.join(format!("{name} {stamp} ({n}).{ext}"));
        n += 1;
    }
    path
}

fn new_path(title: &str, ext: &str) -> PathBuf {
    stamped(folder(), title, ext)
}

/// A new file for something a friend sent.
pub fn received_path(title: &str, ext: &str) -> PathBuf {
    stamped(received_folder(), title, ext)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet(ts: i64, key: bool) -> Packet {
        Packet { data: vec![0], ts, key }
    }

    #[test]
    fn ring_keeps_a_key_frame_before_the_window() {
        let mut r = VideoRing::default();
        for s in 0..100 {
            r.push(packet(s * SEC, s % 2 == 0));
        }
        let first = r.packets.front().unwrap();
        assert!(first.key);
        assert_eq!(first.ts, 68 * SEC, "the key frame at or before 99 - 30 s");
    }

    #[test]
    fn clip_starts_on_a_key_frame_and_ends_in_time() {
        let mut r = VideoRing::default();
        for s in 0..100 {
            r.push(packet(s * SEC, s % 2 == 0));
        }
        let c = r.clip(99 * SEC);
        assert_eq!(c.first().unwrap().ts, 68 * SEC);
        assert_eq!(c.last().unwrap().ts, 99 * SEC);
        let c = r.clip(80 * SEC);
        assert_eq!(c.last().unwrap().ts, 80 * SEC);
    }

    #[test]
    fn a_clip_without_a_key_frame_is_empty() {
        let mut r = VideoRing::default();
        r.push(packet(0, false));
        assert!(r.clip(SEC).is_empty());
    }

    #[test]
    fn silence_fills_the_holes_in_sound() {
        let mut a = AudioRing::default();
        // 100 ms of sound ending at 1 s, then 100 ms ending at 2 s
        a.push(&vec![1000; 9600], SEC);
        a.push(&vec![2000; 9600], 2 * SEC);
        let cut = a.cut(9 * SEC / 10, 2 * SEC);
        assert_eq!(cut.len(), (RATE as usize * 11 / 10) * 2);
        assert_eq!(cut[100], 1000, "first sound");
        assert_eq!(cut[RATE as usize], 0, "the hole");
        assert_eq!(*cut.last().unwrap(), 2000, "second sound");
    }

    #[test]
    fn sound_missing_at_the_edges_is_silence() {
        let mut a = AudioRing::default();
        a.push(&vec![500; 4800], SEC);
        let cut = a.cut(0, 3 * SEC);
        assert_eq!(cut.len(), 3 * RATE as usize * 2);
        assert_eq!(cut[0], 0);
        assert_eq!(cut[(RATE as usize - 100) * 2], 500);
        assert_eq!(*cut.last().unwrap(), 0);
    }

    #[test]
    fn clock_stops_while_paused() {
        let mut c = Clock::new();
        std::thread::sleep(Duration::from_millis(30));
        c.set_paused(true);
        let a = c.now();
        std::thread::sleep(Duration::from_millis(30));
        assert_eq!(c.now(), a);
        c.set_paused(false);
        std::thread::sleep(Duration::from_millis(5));
        let b = c.now();
        assert!(b > a && b - a < 250_000, "the pause isn't counted: {}", b - a);
    }

    #[test]
    fn folder_names() {
        for good in ["Клипы", "My Clips 2", "a.b", "COM"] {
            assert!(folder_name_ok(good), "{good}");
        }
        for bad in ["", "  ", "a/b", r"a\b", "..", "x.", "a:b", "CON", "nul.txt", "COM1", "lpt9", "a?"] {
            assert!(!folder_name_ok(bad), "{bad}");
        }
    }

    #[test]
    fn names_windows_accepts() {
        assert_eq!(safe_name("UFC 5: Ultimate?"), "UFC 5 Ultimate");
        assert_eq!(safe_name("..\\..\\evil"), "....evil".trim_matches('.'));
        assert_eq!(safe_name("  ... "), "Клип");
    }
}

/// Runs on the real screen, GPU and speakers.
/// `cargo test --release clips::live -- --ignored --nocapture`
#[cfg(test)]
mod live {
    use super::*;

    fn cpu_seconds() -> f64 {
        use windows::Win32::Foundation::FILETIME;
        use windows::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};
        let (mut a, mut b, mut k, mut u) = (FILETIME::default(), FILETIME::default(), FILETIME::default(), FILETIME::default());
        unsafe { GetProcessTimes(GetCurrentProcess(), &mut a, &mut b, &mut k, &mut u) }.unwrap();
        let v = |f: FILETIME| (((f.dwHighDateTime as u64) << 32) | f.dwLowDateTime as u64) as f64 / 1e7;
        v(k) + v(u)
    }

    fn wait_saved(rx: std::sync::mpsc::Receiver<Result<Saved, String>>) -> Saved {
        rx.recv_timeout(Duration::from_secs(30)).expect("nothing came back").expect("failed")
    }

    #[test]
    #[ignore]
    fn records_a_clip_and_a_screenshot() {
        let dir = std::env::temp_dir().join("otium-clip-test");
        let _ = std::fs::remove_dir_all(&dir);
        std::env::set_var("OTIUM_CLIPS", &dir);
        let rec = Recorder::start(Arc::new(AtomicIsize::new(0)));
        rec.set_wanted(true);
        // something to hear in the clip
        crate::sound::play(crate::sound::Sfx::Ring);
        std::thread::sleep(Duration::from_secs(2));
        let (c0, t0) = (cpu_seconds(), Instant::now());
        std::thread::sleep(Duration::from_secs(6));
        let cores = std::thread::available_parallelism().map_or(1, |n| n.get()) as f64;
        println!("recording takes {:.1}% of the CPU ({cores} threads)", (cpu_seconds() - c0) / t0.elapsed().as_secs_f64() / cores * 100.0);

        let (tx, rx) = std::sync::mpsc::channel();
        rec.screenshot("Тест", move |r| tx.send(r).unwrap());
        let shot = wait_saved(rx);
        let (tx, rx) = std::sync::mpsc::channel();
        let t = Instant::now();
        rec.save_clip("Тест", move |r| tx.send(r).unwrap());
        let clip = wait_saved(rx);
        println!("saved in {:.0?}: {:?}, {} s", t.elapsed(), clip.path, clip.secs);
        {
            let v = rec.shared.video.lock().unwrap();
            let bytes: usize = v.packets.iter().map(|p| p.data.len()).sum();
            let span = v.packets.back().unwrap().ts - v.packets.front().unwrap().ts;
            println!("{}x{} {} packets, {:.1} fps, {:.1} Mbit/s", v.width, v.height, v.packets.len(), v.packets.len() as f64 / (span as f64 / SEC as f64), bytes as f64 * 8.0 / (span as f64 / SEC as f64) / 1e6);
        }
        let mb = std::fs::metadata(&clip.path).unwrap().len() as f64 / 1e6;
        let info = crate::video::probe(&clip.path).expect("the clip can't be opened");
        println!("{mb:.1} MB, {info:?}");
        assert!(info.frames > 100, "too few frames");
        assert!(info.audio_samples > 48_000 * 5, "too little sound");
        // watching it: played to the end, once, with frames on the way
        let n = Arc::new(std::sync::atomic::AtomicU32::new(0));
        let c = n.clone();
        let t = Instant::now();
        let p = crate::video::Player::start_once(&clip.path, move || {
            c.fetch_add(1, Ordering::Relaxed);
        });
        while !p.ended() && t.elapsed() < Duration::from_secs(20) {
            std::thread::sleep(Duration::from_millis(50));
        }
        let frame = p.take().expect("no frame");
        println!("played in {:.1?}: {} wakes, {}x{}", t.elapsed(), n.load(Ordering::Relaxed), frame.size[0], frame.size[1]);
        assert!(p.ended() && !p.failed() && frame.size[0] == 1920);
        assert!(crate::video::sound_of(&clip.path).len() > 48_000 * 10);
        // the clip's colours are the screen's: the picture path keeps them right
        let out = dir.join("poster.jpg");
        assert!(crate::video::poster(&clip.path, &out, 640), "no poster");
        let a = image::open(&shot.path).unwrap().resize_exact(32, 18, image::imageops::FilterType::Triangle).to_rgb8();
        let b = image::open(&out).unwrap().resize_exact(32, 18, image::imageops::FilterType::Triangle).to_rgb8();
        let diff: f64 = a.pixels().zip(b.pixels()).map(|(x, y)| x.0.iter().zip(y.0.iter()).map(|(p, q)| (*p as f64 - *q as f64).abs()).sum::<f64>()).sum::<f64>() / (32.0 * 18.0 * 3.0);
        let lit = a.pixels().filter(|p| p.0.iter().any(|v| *v > 16)).count();
        println!("screenshot {:?}: {lit}/576 lit, average difference from the clip: {diff:.1} of 255", shot.path);
        assert!(lit > 0, "the screenshot is black");
    }
}

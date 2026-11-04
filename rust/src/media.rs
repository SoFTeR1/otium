//! The Windows "now playing" media session (Spotify, Яндекс Музыка, browsers…) through
//! GlobalSystemMediaTransportControls, polled on a worker thread.

use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

#[derive(Debug, Clone, PartialEq)]
pub struct Music {
    pub title: String,
    pub artist: String,
    pub album: String,
    pub app: String,
    pub playing: bool,
    /// `bytes://` URI of the cover art, once registered with egui
    pub art: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaCmd {
    Poll,
    Toggle,
    Next,
    Prev,
}

struct Session {
    mgr: windows::Media::Control::GlobalSystemMediaTransportControlsSessionManager,
    last_key: String,
    art: Option<String>,
    art_n: u32,
}

impl Session {
    fn new() -> Option<Self> {
        use windows::Media::Control::GlobalSystemMediaTransportControlsSessionManager as Mgr;
        let mgr = Mgr::RequestAsync().ok()?.get().ok()?;
        Some(Self { mgr, last_key: String::new(), art: None, art_n: 0 })
    }

    fn run(&mut self, cmd: MediaCmd, ctx: &egui::Context) -> Option<Music> {
        use windows::Media::Control::GlobalSystemMediaTransportControlsSessionPlaybackStatus as St;
        let s = self.mgr.GetCurrentSession().ok()?;
        let sent = match cmd {
            MediaCmd::Toggle => s.TryTogglePlayPauseAsync().and_then(|o| o.get()).is_ok(),
            MediaCmd::Next => s.TrySkipNextAsync().and_then(|o| o.get()).is_ok(),
            MediaCmd::Prev => s.TrySkipPreviousAsync().and_then(|o| o.get()).is_ok(),
            MediaCmd::Poll => false,
        };
        if sent {
            // players take a moment to report the new state
            std::thread::sleep(Duration::from_millis(350));
        }
        let p = s.TryGetMediaPropertiesAsync().ok()?.get().ok()?;
        let title = p.Title().map(|h| h.to_string()).unwrap_or_default();
        if title.is_empty() {
            return None;
        }
        let artist = p.Artist().map(|h| h.to_string()).unwrap_or_default();
        let key = format!("{title}|{artist}");
        if key != self.last_key {
            self.last_key = key;
            self.art = None;
            if let Some(bytes) = thumbnail(&p) {
                // a fresh URI per track: egui caches images by URI
                self.art_n += 1;
                let uri = format!("bytes://music/{}.png", self.art_n);
                ctx.include_bytes(uri.clone(), bytes);
                self.art = Some(uri);
            }
        }
        let playing = s.GetPlaybackInfo().and_then(|i| i.PlaybackStatus()).is_ok_and(|st| st == St::Playing);
        Some(Music {
            title,
            artist,
            album: p.AlbumTitle().map(|h| h.to_string()).unwrap_or_default(),
            app: s.SourceAppUserModelId().map(|h| h.to_string()).unwrap_or_default(),
            playing,
            art: self.art.clone(),
        })
    }
}

fn thumbnail(p: &windows::Media::Control::GlobalSystemMediaTransportControlsSessionMediaProperties) -> Option<Vec<u8>> {
    use windows::Storage::Streams::DataReader;
    let stream = p.Thumbnail().ok()?.OpenReadAsync().ok()?.get().ok()?;
    let size = stream.Size().ok()? as u32;
    if size == 0 || size > 16 * 1024 * 1024 {
        return None;
    }
    let reader = DataReader::CreateDataReader(&stream).ok()?;
    reader.LoadAsync(size).ok()?.get().ok()?;
    let mut buf = vec![0u8; size as usize];
    reader.ReadBytes(&mut buf).ok()?;
    Some(buf)
}

/// Polls every 2 s (skipped while `idle`, i.e. a game has the screen) and runs commands as they
/// come; `send` gets every change of what's playing.
pub fn start(ctx: egui::Context, cmds: Receiver<MediaCmd>, idle: Arc<AtomicBool>, send: impl Fn(Option<Music>) + Send + 'static) {
    std::thread::Builder::new()
        .name("media".into())
        .spawn(move || {
            unsafe {
                let _ = windows::Win32::System::Com::CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED);
            }
            let mut session: Option<Session> = None;
            let mut last: Option<Music> = None;
            loop {
                let cmd = match cmds.recv_timeout(Duration::from_secs(2)) {
                    Ok(c) => c,
                    Err(RecvTimeoutError::Timeout) if idle.load(Ordering::Relaxed) => continue,
                    Err(RecvTimeoutError::Timeout) => MediaCmd::Poll,
                    Err(RecvTimeoutError::Disconnected) => return,
                };
                if session.is_none() {
                    session = Session::new();
                }
                let now = session.as_mut().and_then(|s| s.run(cmd, &ctx));
                if now != last || cmd != MediaCmd::Poll {
                    last = now.clone();
                    send(now);
                }
            }
        })
        .ok();
}

pub fn channel() -> (Sender<MediaCmd>, Receiver<MediaCmd>) {
    std::sync::mpsc::channel()
}

/// Short player name from the app id the session reports.
pub fn app_name(id: &str) -> String {
    let l = id.to_lowercase();
    for (k, name) in [
        ("spotify", "Spotify"),
        ("yandex", "Яндекс Музыка"),
        ("chrome", "Chrome"),
        ("msedge", "Edge"),
        ("firefox", "Firefox"),
        ("vk", "VK Музыка"),
        ("zune", "Медиаплеер"),
        ("groove", "Медиаплеер"),
    ] {
        if l.contains(k) {
            return name.into();
        }
    }
    id.split(['\\', '!', '.']).filter(|s| !s.is_empty() && *s != "exe").last().unwrap_or("Плеер").to_string()
}

#[cfg(test)]
mod tests {
    use super::app_name;

    #[test]
    fn names() {
        assert_eq!(app_name("Spotify.exe"), "Spotify");
        assert_eq!(app_name("Microsoft.ZuneMusic_8wekyb3d8bbwe!Microsoft.ZuneMusic"), "Медиаплеер");
        assert_eq!(app_name("foobar2000.exe"), "foobar2000");
    }
}

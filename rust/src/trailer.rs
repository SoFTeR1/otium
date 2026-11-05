//! Live backgrounds: a Steam game's trailer, like on a PS5. Steam serves trailers as HLS with
//! fragmented MP4 pieces and the picture separate from the sound, so the first part of the 720p
//! picture stream is simply downloaded and joined into one file, kept in a cache and looped.
//! Blocking: called from worker threads.

use serde_json::Value;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// How much of a trailer is kept (pieces are usually 3 s long).
const MAX_SECONDS: f32 = 45.0;
const MAX_HEIGHT: u32 = 720;
const CACHE_BYTES: u64 = 400 * 1024 * 1024;
/// A game without a trailer is asked about again after this long.
const NONE_FOR: Duration = Duration::from_secs(7 * 24 * 3600);

fn cache_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    base.join("OTIUM").join("Trailers")
}

fn get(url: &str) -> Result<Vec<u8>, String> {
    let r = ureq::get(url).timeout(Duration::from_secs(30)).call().map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    r.into_reader().take(64 * 1024 * 1024).read_to_end(&mut out).map_err(|e| e.to_string())?;
    Ok(out)
}

fn get_text(url: &str) -> Result<String, String> {
    String::from_utf8(get(url)?).map_err(|e| e.to_string())
}

/// The file with this game's trailer: from the cache, or downloaded now. None: it has no trailer
/// (or the network failed, which is tried again next time).
pub fn fetch(appid: &str) -> Option<PathBuf> {
    if appid.is_empty() || !appid.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let dir = cache_dir();
    let file = dir.join(format!("{appid}.mp4"));
    if file.exists() {
        // the cache drops what was shown longest ago
        if let Ok(f) = std::fs::File::options().write(true).open(&file) {
            let _ = f.set_modified(SystemTime::now());
        }
        return Some(file);
    }
    let none = dir.join(format!("{appid}.none"));
    let fresh = std::fs::metadata(&none).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok());
    if fresh.is_some_and(|age| age < NONE_FOR) {
        return None;
    }
    std::fs::create_dir_all(&dir).ok()?;
    let details: Value = serde_json::from_slice(&get(&format!("https://store.steampowered.com/api/appdetails?appids={appid}&filters=movies")).ok()?).ok()?;
    let Some(url) = pick_movie(&details[appid]["data"]["movies"]) else {
        let _ = std::fs::write(&none, b"");
        return None;
    };
    let bytes = download_hls(&url).map_err(|e| eprintln!("trailer {appid}: {e}")).ok()?;
    // written whole or not at all: a half file must never look like a cached one
    let tmp = dir.join(format!("{appid}.part"));
    std::fs::write(&tmp, &bytes).ok()?;
    std::fs::rename(&tmp, &file).ok()?;
    prune(&dir, &file);
    Some(file)
}

/// The store's main trailer, as HLS.
fn pick_movie(movies: &Value) -> Option<String> {
    let list = movies.as_array()?;
    let m = list.iter().find(|m| m["highlight"] == true).or(list.first())?;
    m["hls_h264"].as_str().filter(|s| !s.is_empty()).map(str::to_string)
}

fn download_hls(master_url: &str) -> Result<Vec<u8>, String> {
    let master = get_text(master_url)?;
    let variant = pick_variant(&master).ok_or("no video stream")?;
    let variant_url = join_url(master_url, &variant);
    let list = get_text(&variant_url)?;
    let (init, chunks) = parse_media(&list, MAX_SECONDS).ok_or("no pieces")?;
    let mut out = get(&join_url(&variant_url, &init))?;
    for c in chunks {
        out.extend(get(&join_url(&variant_url, &c))?);
    }
    Ok(out)
}

/// The sharpest stream up to 720p (or the smallest there is).
fn pick_variant(master: &str) -> Option<String> {
    let mut best: Option<(u32, String)> = None;
    let mut lines = master.lines();
    while let Some(l) = lines.next() {
        let Some(info) = l.strip_prefix("#EXT-X-STREAM-INF:") else { continue };
        let Some(uri) = lines.next().map(str::trim).filter(|u| !u.is_empty() && !u.starts_with('#')) else { continue };
        let height = info.split(',').find_map(|p| p.strip_prefix("RESOLUTION=")).and_then(|r| r.split_once('x')).and_then(|(_, h)| h.parse().ok()).unwrap_or(0);
        let better = match &best {
            None => true,
            Some((b, _)) if *b > MAX_HEIGHT => height < *b,
            Some((b, _)) => height <= MAX_HEIGHT && height > *b,
        };
        if better {
            best = Some((height, uri.to_string()));
        }
    }
    best.map(|b| b.1)
}

/// From a media playlist: the init piece, and the pieces making up about `seconds`.
fn parse_media(list: &str, seconds: f32) -> Option<(String, Vec<String>)> {
    let mut init = None;
    let mut chunks = Vec::new();
    let (mut total, mut dur) = (0.0f32, 0.0f32);
    for l in list.lines().map(str::trim) {
        if let Some(m) = l.strip_prefix("#EXT-X-MAP:") {
            init = m.split(',').find_map(|p| p.strip_prefix("URI=")).map(|u| u.trim_matches('"').to_string());
        } else if let Some(d) = l.strip_prefix("#EXTINF:") {
            dur = d.trim_end_matches(',').split(',').next().and_then(|x| x.parse().ok()).unwrap_or(0.0);
        } else if !l.is_empty() && !l.starts_with('#') {
            if total >= seconds {
                break;
            }
            chunks.push(l.to_string());
            total += dur;
        }
    }
    (!chunks.is_empty()).then_some((init?, chunks))
}

/// A playlist's relative link against the playlist's own address.
fn join_url(base: &str, rel: &str) -> String {
    if rel.starts_with("http://") || rel.starts_with("https://") {
        return rel.to_string();
    }
    let no_query = base.split('?').next().unwrap_or(base);
    match no_query.rsplit_once('/') {
        Some((dir, _)) => format!("{dir}/{rel}"),
        None => rel.to_string(),
    }
}

/// Keeps the cache under its limit, dropping what was shown longest ago (never `keep`).
fn prune(dir: &Path, keep: &Path) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut files: Vec<(SystemTime, u64, PathBuf)> = rd
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "mp4"))
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            Some((m.modified().ok()?, m.len(), e.path()))
        })
        .collect();
    let mut total: u64 = files.iter().map(|f| f.1).sum();
    files.sort_by_key(|f| f.0);
    for (_, len, path) in files {
        if total <= CACHE_BYTES {
            break;
        }
        if path != keep && std::fs::remove_file(&path).is_ok() {
            total -= len;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MASTER: &str = "#EXTM3U\n#EXT-X-MEDIA:TYPE=AUDIO,URI=\"a.m3u8\"\n#EXT-X-STREAM-INF:BANDWIDTH=5800000,CODECS=\"avc1.640029,mp4a.40.2\",RESOLUTION=1920x1080,FRAME-RATE=30\nv0.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=2600000,RESOLUTION=1280x720\nv1.m3u8\n#EXT-X-STREAM-INF:BANDWIDTH=1400000,RESOLUTION=854x480\nv2.m3u8\n";

    #[test]
    fn takes_720p() {
        assert_eq!(pick_variant(MASTER).as_deref(), Some("v1.m3u8"));
    }

    #[test]
    fn only_big_streams_take_the_smallest() {
        let m = "#EXT-X-STREAM-INF:RESOLUTION=3840x2160\na\n#EXT-X-STREAM-INF:RESOLUTION=1920x1080\nb\n";
        assert_eq!(pick_variant(m).as_deref(), Some("b"));
    }

    #[test]
    fn pieces_up_to_the_limit() {
        let l = "#EXTM3U\n#EXT-X-MAP:URI=\"d/init.m4s\"\n#EXTINF:3\nd/1.m4s\n#EXTINF:3,\nd/2.m4s\n#EXTINF:3\nd/3.m4s\n#EXT-X-ENDLIST\n";
        let (init, c) = parse_media(l, 6.0).unwrap();
        assert_eq!(init, "d/init.m4s");
        assert_eq!(c, ["d/1.m4s", "d/2.m4s"]);
        // no init piece: not the fragmented kind this can join
        assert!(parse_media("#EXTINF:3\na.ts\n", 9.0).is_none());
    }

    #[test]
    fn links() {
        assert_eq!(join_url("https://x/a/m.m3u8?t=1", "v.m3u8"), "https://x/a/v.m3u8");
        assert_eq!(join_url("https://x/a/v.m3u8", "d/1.m4s"), "https://x/a/d/1.m4s");
        assert_eq!(join_url("https://x/a/v.m3u8", "https://y/z"), "https://y/z");
    }

    #[test]
    fn main_trailer_first() {
        let m: Value = serde_json::json!([{ "hls_h264": "a" }, { "highlight": true, "hls_h264": "b" }]);
        assert_eq!(pick_movie(&m).as_deref(), Some("b"));
        assert_eq!(pick_movie(&serde_json::json!([])), None);
        assert_eq!(pick_movie(&serde_json::json!([{ "id": 1 }])), None);
    }

    #[test]
    fn odd_ids_are_refused() {
        assert!(fetch("../../x").is_none());
        assert!(fetch("").is_none());
    }
}

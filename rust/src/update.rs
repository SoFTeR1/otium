//! Updates from GitHub Releases: the latest release of SoFTeR1/otium carries `OTIUM-Setup-X.exe`
//! (the same exe as OTIUM itself, see installer.rs). When it's newer than this build, OTIUM shows
//! a banner; installing downloads it and lets it replace the running exe.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

const LATEST: &str = "https://api.github.com/repos/SoFTeR1/otium/releases/latest";

#[derive(Debug, Clone)]
pub struct Release {
    pub version: String,
    pub url: String,
    pub name: String,
    pub size: u64,
}

pub fn current() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// "stable 3.0.0" / "preview 3.1.0-alpha.2"
pub fn label() -> String {
    let v = current();
    if v.contains('-') { format!("preview {v}") } else { format!("stable {v}") }
}

/// (major, minor, patch, pre) — a pre-release sorts before its release.
fn parse(v: &str) -> Option<(u64, u64, u64, Option<String>)> {
    let v = v.trim().trim_start_matches('v');
    let (main, pre) = match v.split_once('-') {
        Some((m, p)) => (m, Some(p.to_string())),
        None => (v, None),
    };
    let mut it = main.split('.').map(|n| n.parse::<u64>().ok());
    Some((it.next()??, it.next().flatten().unwrap_or(0), it.next().flatten().unwrap_or(0), pre))
}

pub fn newer(candidate: &str, than: &str) -> bool {
    let (Some(a), Some(b)) = (parse(candidate), parse(than)) else { return false };
    let base = (a.0, a.1, a.2).cmp(&(b.0, b.1, b.2));
    match base {
        std::cmp::Ordering::Greater => true,
        std::cmp::Ordering::Less => false,
        // same numbers: a release beats its pre-releases; two pre-releases compare by their tail
        std::cmp::Ordering::Equal => match (&a.3, &b.3) {
            (None, Some(_)) => true,
            (Some(x), Some(y)) => pre_key(x) > pre_key(y),
            _ => false,
        },
    }
}

fn pre_key(p: &str) -> (String, u64) {
    let (name, n) = p.rsplit_once('.').unwrap_or((p, "0"));
    (name.to_string(), n.parse().unwrap_or(0))
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(10)).timeout_read(Duration::from_secs(30)).user_agent("OTIUM-updater").build()
}

/// The latest release if it's newer than this build and has an installer.
pub fn check() -> Option<Release> {
    let v: serde_json::Value = agent().get(LATEST).set("Accept", "application/vnd.github+json").call().ok()?.into_json().ok()?;
    let tag = v["tag_name"].as_str()?;
    if v["prerelease"].as_bool() == Some(true) || v["draft"].as_bool() == Some(true) || !newer(tag, current()) {
        return None;
    }
    let asset = v["assets"].as_array()?.iter().find(|a| {
        let n = a["name"].as_str().unwrap_or("").to_lowercase();
        n.starts_with("otium-setup-") && n.ends_with(".exe")
    })?;
    Some(Release {
        version: tag.trim_start_matches('v').to_string(),
        url: asset["browser_download_url"].as_str()?.to_string(),
        name: asset["name"].as_str()?.to_string(),
        size: asset["size"].as_u64().unwrap_or(0),
    })
}

/// Downloads the installer next to the temp folder, reporting 0..=100.
pub fn download(r: &Release, progress: impl Fn(u8)) -> Result<PathBuf, String> {
    let dir = std::env::temp_dir().join("otium-update");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let out = dir.join(&r.name);
    let resp = agent().get(&r.url).call().map_err(|e| format!("Не удалось скачать обновление: {e}"))?;
    let total = resp.header("Content-Length").and_then(|v| v.parse::<u64>().ok()).unwrap_or(r.size).max(1);
    let mut reader = resp.into_reader();
    let tmp = out.with_extension("part");
    let mut file = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; 256 * 1024];
    let mut got = 0u64;
    let mut last = 255u8;
    loop {
        let n = reader.read(&mut buf).map_err(|e| format!("Обрыв при скачивании: {e}"))?;
        if n == 0 {
            break;
        }
        std::io::Write::write_all(&mut file, &buf[..n]).map_err(|e| e.to_string())?;
        got += n as u64;
        let pct = (got * 100 / total).min(100) as u8;
        if pct != last {
            last = pct;
            progress(pct);
        }
    }
    drop(file);
    if r.size > 0 && got != r.size {
        return Err("Обновление скачалось не полностью — попробуйте ещё раз".into());
    }
    let _ = std::fs::remove_file(&out);
    std::fs::rename(&tmp, &out).map_err(|e| e.to_string())?;
    Ok(out)
}

/// Starts the downloaded installer to replace this exe once we've exited; the caller quits.
pub fn apply(installer: &Path) -> Result<(), String> {
    let me = std::env::current_exe().map_err(|e| e.to_string())?;
    std::process::Command::new(installer)
        .arg("--self-update")
        .arg(&me)
        .arg(std::process::id().to_string())
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Не удалось запустить обновление: {e}"))
}

#[cfg(test)]
mod tests {
    use super::newer;

    #[test]
    fn versions() {
        assert!(newer("v3.0.1", "3.0.0"));
        assert!(newer("3.1.0", "3.0.9"));
        assert!(!newer("3.0.0", "3.0.0"));
        assert!(!newer("2.1.1", "3.0.0"));
        assert!(newer("3.0.0", "3.0.0-alpha.9"));
        assert!(!newer("3.0.0-alpha.9", "3.0.0"));
        assert!(newer("3.0.0-alpha.10", "3.0.0-alpha.9"));
    }

    /// What GitHub says right now: `cargo test -- --ignored latest_release --nocapture`
    #[test]
    #[ignore]
    fn latest_release() {
        println!("current {}, update: {:?}", super::current(), super::check());
    }
}

//! The Store: a catalog of free games and PC ports, installed from their authors' GitHub releases.
//! OTIUM never carries or fetches anyone's game files: a port that needs a copy of the original
//! game says so, and asks for it itself when it starts. Blocking: runs on worker threads.

use serde::Deserialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

const REMOTE: &str = "https://raw.githubusercontent.com/SoFTeR1/otium-release/main/catalog.json";
const BUILT_IN: &str = include_str!("../assets/catalog.json");
const MARKER: &str = ".otium-store";

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct Item {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub about: String,
    /// what the user must have themselves
    #[serde(default)]
    pub needs: Option<String>,
    /// owner/name on GitHub
    #[serde(default)]
    pub repo: String,
    /// how the name of the Windows archive in a release ends
    #[serde(default)]
    pub asset: String,
    /// the program, relative to the unpacked folder
    #[serde(default)]
    pub exe: String,
    /// a game that runs through a launcher: that launcher's id (installing means installing it)
    #[serde(default)]
    pub via: Option<String>,
    /// a launcher, not shown as a tile of its own
    #[serde(default)]
    pub hidden: bool,
    #[serde(default)]
    pub args: String,
    /// it updates itself (OTIUM leaves that to it)
    #[serde(default)]
    pub self_updates: bool,
    pub color: [u8; 3],
}

impl Item {
    /// The catalog comes from the network: nothing in it may point outside the game's folder.
    fn is_safe(&self) -> bool {
        let id_ok = !self.id.is_empty() && self.id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
        let word = |s: &str| !s.is_empty() && s != "." && s != ".." && s.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c));
        if let Some(v) = &self.via {
            return id_ok && word(v);
        }
        let repo_ok = self.repo.split_once('/').is_some_and(|(a, b)| word(a) && word(b));
        let exe = Path::new(&self.exe);
        let exe_ok = self.exe.to_ascii_lowercase().ends_with(".exe") && exe.components().all(|c| matches!(c, Component::Normal(_)));
        id_ok && repo_ok && exe_ok && self.asset.ends_with(".zip")
    }
}

fn parse(text: &str) -> Vec<Item> {
    let items: Vec<Item> = serde_json::from_str(text).unwrap_or_default();
    items.into_iter().filter(Item::is_safe).collect()
}

fn base_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA").map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    base.join("OTIUM")
}

/// Where games go unless the user picked another folder.
pub fn default_root() -> PathBuf {
    base_dir().join("Games")
}

/// The catalog at hand: last downloaded, or the one built in.
pub fn catalog() -> Vec<Item> {
    let cached = std::fs::read_to_string(base_dir().join("catalog.json")).map(|t| parse(&t)).unwrap_or_default();
    if cached.is_empty() {
        parse(BUILT_IN)
    } else {
        cached
    }
}

/// The newest catalog from the repository (kept for next time), when it can be had and read.
pub fn fetch_catalog() -> Option<Vec<Item>> {
    let text = agent().get(REMOTE).call().ok()?.into_string().ok()?;
    let items = parse(&text);
    if items.is_empty() {
        return None;
    }
    let _ = std::fs::create_dir_all(base_dir());
    let _ = std::fs::write(base_dir().join("catalog.json"), text);
    Some(items)
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new().user_agent("OTIUM").timeout_connect(Duration::from_secs(15)).timeout_read(Duration::from_secs(60)).build()
}

#[derive(Debug, Clone)]
pub struct Release {
    pub version: String,
    url: String,
    name: String,
    size: u64,
    sums_url: Option<String>,
}

/// The archive in the latest release of the game's repository.
pub fn latest(item: &Item) -> Result<Release, String> {
    let url = format!("https://api.github.com/repos/{}/releases/latest", item.repo);
    let v: Value = match agent().get(&url).call() {
        Ok(r) => r.into_json().map_err(|e| e.to_string())?,
        // GitHub allows 60 questions an hour per address, and behind a VPN that address is shared
        Err(ureq::Error::Status(403 | 429, _)) => from_release_page(&item.repo)?,
        Err(e) => return Err(format!("GitHub не ответил: {e}")),
    };
    pick_asset(&v, &item.asset).ok_or_else(|| format!("В последнем релизе нет файла «*{}»", item.asset))
}

/// The same facts from the release's web page, which has no such limit. Sizes aren't known there.
fn from_release_page(repo: &str) -> Result<Value, String> {
    let err = |e: &dyn std::fmt::Display| format!("GitHub не ответил: {e}");
    let page = agent().get(&format!("https://github.com/{repo}/releases/latest")).call().map_err(|e| err(&e))?;
    // where it ended up: a project may have moved to another owner
    let (home, tag) = page.get_url().rsplit_once("/releases/tag/").map(|(h, t)| (h.to_string(), t.to_string())).ok_or_else(|| err(&"нет релизов"))?;
    let repo = home.strip_prefix("https://github.com/").unwrap_or(repo).to_string();
    let html = agent().get(&format!("https://github.com/{repo}/releases/expanded_assets/{tag}")).call().map_err(|e| err(&e))?.into_string().map_err(|e| err(&e))?;
    Ok(release_from_html(&repo, &tag, &html))
}

/// Builds what the API would have said from the links on the page.
fn release_from_html(repo: &str, tag: &str, html: &str) -> Value {
    let prefix = format!("/{repo}/releases/download/{tag}/");
    let assets: Vec<Value> = html
        .split("href=\"")
        .skip(1)
        .filter_map(|p| p.split('"').next())
        .filter_map(|link| link.strip_prefix(&prefix))
        .map(|name| serde_json::json!({ "name": name, "browser_download_url": format!("https://github.com{prefix}{name}"), "size": 0 }))
        .collect();
    serde_json::json!({ "tag_name": tag, "assets": assets })
}

fn pick_asset(release: &Value, suffix: &str) -> Option<Release> {
    let assets = release["assets"].as_array()?;
    let a = assets.iter().find(|a| a["name"].as_str().is_some_and(|n| n.ends_with(suffix)))?;
    let sums = assets.iter().find(|a| a["name"].as_str().is_some_and(|n| n.to_ascii_lowercase().contains("sha256")));
    Some(Release {
        version: release["tag_name"].as_str()?.to_string(),
        url: a["browser_download_url"].as_str()?.to_string(),
        name: a["name"].as_str()?.to_string(),
        size: a["size"].as_u64().unwrap_or(0),
        sums_url: sums.and_then(|s| s["browser_download_url"].as_str()).map(str::to_string),
    })
}

/// A checksum list's entry for one file ("<hash>  <name>" or "<hash> *<name>").
fn sum_for(list: &str, name: &str) -> Option<String> {
    list.lines().find_map(|l| {
        let (hash, file) = l.trim().split_once(char::is_whitespace)?;
        (file.trim().trim_start_matches('*') == name && hash.len() == 64).then(|| hash.to_ascii_lowercase())
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Download,
    Unpack,
}

#[derive(Debug, Clone)]
pub struct Installed {
    pub version: String,
    pub dir: PathBuf,
    pub exe: PathBuf,
}

/// Installs or updates a game under `root`. On an update new files go over the old ones, so what the
/// game wrote into its folder (saves, settings) stays.
pub fn install(item: &Item, root: &Path, progress: impl Fn(Stage, u8)) -> Result<Installed, String> {
    if !item.is_safe() {
        return Err("Запись каталога не прошла проверку".into());
    }
    let rel = latest(item)?;
    let fail = |what: &str, e: &dyn std::fmt::Display| format!("{what}: {e}");
    std::fs::create_dir_all(root).map_err(|e| fail("Не удалось создать папку", &e))?;
    if let Some(free) = crate::win::disk_free(root) {
        // room for the archive and what's inside it
        if free < rel.size.saturating_mul(3) {
            return Err(format!("Мало места на диске: нужно примерно {} МБ", rel.size.saturating_mul(3) / 1_048_576));
        }
    }
    let archive = root.join(format!(".{}.zip.part", item.id));
    let res = download(&rel, &archive, &progress).and_then(|_| {
        let new = root.join(format!(".{}.new", item.id));
        let _ = std::fs::remove_dir_all(&new);
        let r = unpack(&archive, &new, &progress).and_then(|_| place(item, &new, root));
        let _ = std::fs::remove_dir_all(&new);
        r
    });
    let _ = std::fs::remove_file(&archive);
    let dir = res?;
    let exe = dir.join(&item.exe);
    Ok(Installed { version: rel.version, dir, exe })
}

fn download(rel: &Release, to: &Path, progress: &impl Fn(Stage, u8)) -> Result<(), String> {
    let expected = rel.sums_url.as_ref().and_then(|u| agent().get(u).call().ok()?.into_string().ok()).and_then(|l| sum_for(&l, &rel.name));
    let resp = agent().get(&rel.url).call().map_err(|e| format!("Не удалось скачать: {e}"))?;
    // a size from the release page isn't known: the server tells it now
    let size = if rel.size > 0 { rel.size } else { resp.header("Content-Length").and_then(|v| v.parse().ok()).unwrap_or(0) };
    let mut reader = resp.into_reader();
    let mut file = std::fs::File::create(to).map_err(|e| e.to_string())?;
    let mut hash = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    let (mut got, mut last) = (0u64, 255u8);
    loop {
        let n = reader.read(&mut buf).map_err(|e| format!("Обрыв при скачивании: {e}"))?;
        if n == 0 {
            break;
        }
        file.write_all(&buf[..n]).map_err(|e| format!("Не удалось записать: {e}"))?;
        hash.update(&buf[..n]);
        got += n as u64;
        let pct = (got * 100 / size.max(1)).min(100) as u8;
        if pct != last {
            last = pct;
            progress(Stage::Download, pct);
        }
    }
    if size > 0 && got != size {
        return Err("Файл скачался не полностью — попробуйте ещё раз".into());
    }
    if let Some(want) = expected {
        let have: String = hash.finalize().iter().map(|b| format!("{b:02x}")).collect();
        if have != want {
            return Err("Контрольная сумма не совпала: файл повреждён или подменён".into());
        }
    }
    Ok(())
}

/// The folder every entry of the archive sits in, if there is exactly one.
fn single_top(names: &[PathBuf]) -> Option<PathBuf> {
    let mut top = None;
    for n in names {
        let mut c = n.components();
        let first = c.next()?;
        // a file at the top: nothing to strip
        c.next()?;
        match &top {
            None => top = Some(first),
            Some(t) if *t == first => {}
            Some(_) => return None,
        }
    }
    top.map(|t| PathBuf::from(t.as_os_str()))
}

fn unpack(archive: &Path, to: &Path, progress: &impl Fn(Stage, u8)) -> Result<(), String> {
    let file = std::fs::File::open(archive).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipArchive::new(std::io::BufReader::new(file)).map_err(|e| format!("Архив повреждён: {e}"))?;
    // enclosed_name refuses entries that climb out of the folder
    let mut names = Vec::new();
    let mut total = 0u64;
    for i in 0..zip.len() {
        let f = zip.by_index(i).map_err(|e| format!("Архив повреждён: {e}"))?;
        total += f.size();
        if !f.is_dir() {
            names.extend(f.enclosed_name());
        }
    }
    if crate::win::disk_free(to.parent().unwrap_or(to)).is_some_and(|free| free < total) {
        return Err(format!("Мало места на диске: нужно {} МБ", total / 1_048_576));
    }
    let top = single_top(&names);
    let (mut done, mut last) = (0u64, 255u8);
    for i in 0..zip.len() {
        let mut f = zip.by_index(i).map_err(|e| format!("Архив повреждён: {e}"))?;
        let Some(name) = f.enclosed_name() else { continue };
        let rel = match &top {
            Some(t) => match name.strip_prefix(t) {
                Ok(r) => r.to_path_buf(),
                Err(_) => continue,
            },
            None => name,
        };
        if rel.as_os_str().is_empty() {
            continue;
        }
        let out = to.join(&rel);
        if f.is_dir() {
            std::fs::create_dir_all(&out).map_err(|e| format!("Не удалось распаковать: {e}"))?;
            continue;
        }
        if let Some(p) = out.parent() {
            std::fs::create_dir_all(p).map_err(|e| format!("Не удалось распаковать: {e}"))?;
        }
        let mut w = std::fs::File::create(&out).map_err(|e| format!("Не удалось распаковать: {e}"))?;
        std::io::copy(&mut f, &mut w).map_err(|e| format!("Не удалось распаковать: {e}"))?;
        done += f.size();
        let pct = (done * 100 / total.max(1)).min(100) as u8;
        if pct != last {
            last = pct;
            progress(Stage::Unpack, pct);
        }
    }
    Ok(())
}

/// Moves the unpacked files into the game's folder, once they turn out to hold the program.
fn place(item: &Item, new: &Path, root: &Path) -> Result<PathBuf, String> {
    if !new.join(&item.exe).is_file() {
        return Err(format!("В архиве нет {}: каталог устарел", item.exe));
    }
    let dir = root.join(&item.id);
    let err = |e: std::io::Error| format!("Не удалось установить: {e}");
    if dir.exists() {
        overlay(new, &dir).map_err(err)?;
    } else {
        std::fs::rename(new, &dir).map_err(err)?;
    }
    std::fs::write(dir.join(MARKER), &item.id).map_err(err)?;
    Ok(dir)
}

fn overlay(from: &Path, to: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let target = to.join(e.file_name());
        if e.file_type()?.is_dir() {
            overlay(&e.path(), &target)?;
        } else {
            std::fs::rename(e.path(), &target)?;
        }
    }
    Ok(())
}

/// The folders inside `dir` to offer for installing, by name (hidden and system ones aren't).
pub fn subfolders(dir: &Path) -> Vec<PathBuf> {
    use std::os::windows::fs::MetadataExt;
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<PathBuf> = rd
        .flatten()
        .filter(|e| e.metadata().is_ok_and(|m| m.is_dir() && m.file_attributes() & 0x6 == 0))
        .map(|e| e.path())
        .collect();
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

/// Deletes an installed game's folder, only if OTIUM's mark for this very game is in it.
pub fn remove(id: &str, dir: &Path) -> Result<(), String> {
    let mark = std::fs::read_to_string(dir.join(MARKER)).unwrap_or_default();
    if mark.trim() != id {
        return Err("Эта папка не похожа на установленную игру — не трогаю её".into());
    }
    std::fs::remove_dir_all(dir).map_err(|e| format!("Не удалось удалить: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item() -> Item {
        parse(BUILT_IN).into_iter().next().unwrap()
    }

    #[test]
    fn built_in_catalog_is_whole() {
        let c = parse(BUILT_IN);
        assert_eq!(c.len(), 15, "a record failed the checks");
        assert!(c.iter().any(|i| i.id == "portheon" && i.self_updates && i.hidden));
        assert!(c.iter().any(|i| i.id == "supertuxkart" && i.needs.is_none()));
        // UFC 1-6 and Undisputed 3 all lead to a launcher that is in the catalog
        let via: Vec<_> = c.iter().filter(|i| i.via.is_some()).collect();
        assert_eq!(via.len(), 7);
        for i in via {
            assert!(!i.hidden && c.iter().any(|l| Some(&l.id) == i.via.as_ref() && l.hidden && !l.repo.is_empty()));
        }
    }

    #[test]
    fn hostile_records_are_dropped() {
        let bad = |f: &dyn Fn(&mut Item)| {
            let mut i = item();
            f(&mut i);
            !i.is_safe()
        };
        assert!(item().is_safe());
        assert!(bad(&|i| i.id = "../../Windows".into()));
        assert!(bad(&|i| i.exe = "../../evil.exe".into()));
        assert!(bad(&|i| i.exe = r"C:\Windows\evil.exe".into()));
        assert!(bad(&|i| i.exe = "readme.txt".into()));
        assert!(bad(&|i| i.repo = "a/../../b".into()));
        assert!(bad(&|i| i.asset = "installer.exe".into()));
        assert!(bad(&|i| i.via = Some("../x".into())));
    }

    #[test]
    fn broken_catalog_gives_nothing() {
        assert!(parse("<html>").is_empty());
    }

    #[test]
    fn right_asset_and_checksums() {
        let r = serde_json::json!({ "tag_name": "5.0", "assets": [
            { "name": "P-5.0-Linux-x64.tar.gz", "browser_download_url": "l", "size": 1 },
            { "name": "P-5.0-Windows-x64.zip", "browser_download_url": "w", "size": 2 },
            { "name": "SHA256SUMS.txt", "browser_download_url": "s", "size": 3 },
        ]});
        let a = pick_asset(&r, "-Windows-x64.zip").unwrap();
        assert_eq!((a.url.as_str(), a.size, a.version.as_str()), ("w", 2, "5.0"));
        assert_eq!(a.sums_url.as_deref(), Some("s"));
        assert!(pick_asset(&r, "-Mac.zip").is_none());
    }

    #[test]
    fn release_page_gives_the_same_facts() {
        let html = r#"<a href="/o/r/releases/download/5.0/G-5.0-Windows-x64.zip" rel="nofollow"><a href="/o/r/releases/download/5.0/SHA256SUMS.txt"><a href="/o/r/archive/refs/tags/5.0.zip"><a href="https://elsewhere/x.zip">"#;
        let v = release_from_html("o/r", "5.0", html);
        let a = pick_asset(&v, "-Windows-x64.zip").unwrap();
        assert_eq!(a.version, "5.0");
        assert_eq!(a.url, "https://github.com/o/r/releases/download/5.0/G-5.0-Windows-x64.zip");
        assert_eq!(a.sums_url.as_deref(), Some("https://github.com/o/r/releases/download/5.0/SHA256SUMS.txt"));
        assert!(pick_asset(&v, ".tar.gz").is_none());
    }

    #[test]
    fn checksum_lines() {
        let h = "e".repeat(64);
        let list = format!("{h}  a.zip\n{} *b.zip\n", "F".repeat(64));
        assert_eq!(sum_for(&list, "a.zip"), Some(h));
        assert_eq!(sum_for(&list, "b.zip"), Some("f".repeat(64)));
        assert_eq!(sum_for(&list, "c.zip"), None);
    }

    #[test]
    fn folder_names() {
        for good in ["Игры", "My Games 2", "a.b", "COM"] {
            assert!(folder_name_ok(good), "{good}");
        }
        for bad in ["", "  ", "a/b", r"a\b", "..", "x.", "a:b", "CON", "nul.txt", "COM1", "lpt9", "a?"] {
            assert!(!folder_name_ok(bad), "{bad}");
        }
    }

    #[test]
    fn folders_on_offer() {
        let d = tmp("sub");
        std::fs::create_dir_all(d.join("b")).unwrap();
        std::fs::create_dir_all(d.join("A")).unwrap();
        std::fs::write(d.join("file.txt"), "x").unwrap();
        let names: Vec<_> = subfolders(&d).iter().map(|p| p.file_name().unwrap().to_string_lossy().to_string()).collect();
        assert_eq!(names, ["A", "b"]);
        assert!(subfolders(&d.join("missing")).is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn top_folder() {
        let p = |v: &[&str]| v.iter().map(PathBuf::from).collect::<Vec<_>>();
        assert_eq!(single_top(&p(&["G-5/a.exe", "G-5/b/c.dll"])), Some(PathBuf::from("G-5")));
        assert_eq!(single_top(&p(&["a.exe", "b/c.dll"])), None);
        assert_eq!(single_top(&p(&["a/x", "b/y"])), None);
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("otium-store-test-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn unpack_strips_the_top_folder_and_refuses_to_climb() {
        let d = tmp("unpack");
        let z = d.join("a.zip");
        let mut w = zip::ZipWriter::new(std::fs::File::create(&z).unwrap());
        let o: zip::write::SimpleFileOptions = Default::default();
        w.start_file("G-5/game.exe", o).unwrap();
        w.write_all(b"exe").unwrap();
        w.start_file("G-5/data/x.txt", o).unwrap();
        w.write_all(b"data").unwrap();
        w.start_file("../../evil.txt", o).unwrap();
        w.write_all(b"evil").unwrap();
        w.finish().unwrap();
        let out = d.join("out");
        unpack(&z, &out, &|_, _| {}).unwrap();
        // the climbing entry is skipped (and doesn't count as a file when looking for the top folder)
        assert_eq!(std::fs::read(out.join("game.exe")).unwrap(), b"exe");
        assert_eq!(std::fs::read(out.join("data/x.txt")).unwrap(), b"data");
        assert!(!d.join("evil.txt").exists() && !out.join("evil.txt").exists());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn update_keeps_what_the_game_wrote_and_removal_needs_the_mark() {
        let d = tmp("place");
        let mut i = item();
        i.id = "game".into();
        i.exe = "g.exe".into();
        let new = d.join("new");
        std::fs::create_dir_all(new.join("data")).unwrap();
        std::fs::write(new.join("g.exe"), "v1").unwrap();
        let dir = place(&i, &new, &d).unwrap();
        std::fs::write(dir.join("save.dat"), "mine").unwrap();
        std::fs::create_dir_all(&new).unwrap();
        std::fs::write(new.join("g.exe"), "v2").unwrap();
        place(&i, &new, &d).unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("g.exe")).unwrap(), "v2");
        assert_eq!(std::fs::read_to_string(dir.join("save.dat")).unwrap(), "mine");
        // an archive without the program installs nothing
        let empty = d.join("empty");
        std::fs::create_dir_all(&empty).unwrap();
        i.id = "other".into();
        assert!(place(&i, &empty, &d).is_err());
        assert!(!d.join("other").exists());
        // removal
        assert!(remove("wrong", &dir).is_err());
        assert!(dir.exists());
        let foreign = d.join("foreign");
        std::fs::create_dir_all(&foreign).unwrap();
        assert!(remove("game", &foreign).is_err());
        assert!(foreign.exists());
        remove("game", &dir).unwrap();
        assert!(!dir.exists());
        let _ = std::fs::remove_dir_all(&d);
    }
}

#[cfg(test)]
mod live {
    use super::*;

    /// cargo test --release store_install -- --ignored --nocapture
    /// Really installs the smallest game into a temp folder, then removes it.
    #[test]
    #[ignore]
    fn store_install() {
        let item = parse(BUILT_IN).into_iter().find(|i| i.id == "fheroes2").unwrap();
        let root = std::env::temp_dir().join("otium-store-live");
        let _ = std::fs::remove_dir_all(&root);
        let res = install(&item, &root, |s, p| {
            if p % 50 == 0 {
                println!("{s:?} {p}%");
            }
        })
        .unwrap();
        println!("{res:?}");
        assert!(res.exe.is_file());
        remove(&item.id, &res.dir).unwrap();
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Every catalog record still matches its latest release.
    #[test]
    #[ignore]
    fn store_releases() {
        for i in parse(BUILT_IN).into_iter().filter(|i| i.via.is_none()) {
            let r = latest(&i).unwrap_or_else(|e| panic!("{}: {e}", i.id));
            println!("{}: {} {} ({} MB), checksums: {}", i.id, r.version, r.name, r.size / 1_048_576, r.sums_url.is_some());
        }
    }
}

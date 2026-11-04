//! The local Steam install: installed games, their art in Steam's cache, and playtime
//! the Steam client recorded.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

pub const CDN: &str = "https://shared.cloudflare.steamstatic.com/store_item_assets/steam/apps";
const STEAMID_BASE: u64 = 76561197960265728;

// ---------- Valve KeyValues (.vdf / .acf) ----------

#[derive(Debug, Clone)]
pub enum Vdf {
    Str(String),
    Obj(Vec<(String, Vdf)>),
}

impl Vdf {
    pub fn parse(text: &str) -> Vdf {
        let chars: Vec<char> = text.chars().collect();
        let mut i = 0;
        Vdf::Obj(parse_obj(&chars, &mut i))
    }

    /// Case-insensitive path lookup: `v.get(&["AppState", "name"])`.
    pub fn get(&self, path: &[&str]) -> Option<&Vdf> {
        let mut cur = self;
        for key in path {
            let Vdf::Obj(items) = cur else { return None };
            cur = &items.iter().find(|(k, _)| k.eq_ignore_ascii_case(key))?.1;
        }
        Some(cur)
    }

    pub fn str(&self, path: &[&str]) -> Option<&str> {
        match self.get(path)? {
            Vdf::Str(s) => Some(s),
            Vdf::Obj(_) => None,
        }
    }

    pub fn entries(&self) -> &[(String, Vdf)] {
        match self {
            Vdf::Obj(items) => items,
            Vdf::Str(_) => &[],
        }
    }
}

fn skip_ws(c: &[char], i: &mut usize) {
    while *i < c.len() {
        match c[*i] {
            ' ' | '\t' | '\r' | '\n' => *i += 1,
            '/' if c.get(*i + 1) == Some(&'/') => {
                while *i < c.len() && c[*i] != '\n' {
                    *i += 1;
                }
            }
            _ => break,
        }
    }
}

fn parse_str(c: &[char], i: &mut usize) -> String {
    let mut s = String::new();
    if c.get(*i) == Some(&'"') {
        *i += 1;
        while *i < c.len() && c[*i] != '"' {
            if c[*i] == '\\' && *i + 1 < c.len() {
                s.push(c[*i + 1]);
                *i += 2;
            } else {
                s.push(c[*i]);
                *i += 1;
            }
        }
        *i += 1;
    } else {
        while *i < c.len() && !c[*i].is_whitespace() && !matches!(c[*i], '{' | '}' | '"') {
            s.push(c[*i]);
            *i += 1;
        }
    }
    s
}

fn parse_obj(c: &[char], i: &mut usize) -> Vec<(String, Vdf)> {
    let mut out = Vec::new();
    loop {
        skip_ws(c, i);
        if *i >= c.len() {
            return out;
        }
        if c[*i] == '}' {
            *i += 1;
            return out;
        }
        let key = parse_str(c, i);
        skip_ws(c, i);
        if c.get(*i) == Some(&'{') {
            *i += 1;
            out.push((key, Vdf::Obj(parse_obj(c, i))));
        } else if *i < c.len() {
            out.push((key, Vdf::Str(parse_str(c, i))));
        } else {
            return out;
        }
    }
}

fn read_vdf(path: &Path) -> Option<Vdf> {
    let bytes = std::fs::read(path).ok()?;
    Some(Vdf::parse(&String::from_utf8_lossy(&bytes)))
}

// ---------- install ----------

pub fn root() -> Option<PathBuf> {
    use winreg::enums::{HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE};
    use winreg::RegKey;
    let hkcu = RegKey::predef(HKEY_CURRENT_USER).open_subkey("Software\\Valve\\Steam");
    let from_hkcu = hkcu.ok().and_then(|k| k.get_value::<String, _>("SteamPath").ok());
    let path = from_hkcu.or_else(|| {
        RegKey::predef(HKEY_LOCAL_MACHINE)
            .open_subkey("SOFTWARE\\WOW6432Node\\Valve\\Steam")
            .ok()
            .and_then(|k| k.get_value::<String, _>("InstallPath").ok())
    })?;
    let p = PathBuf::from(path.replace('/', "\\"));
    p.exists().then_some(p)
}

fn libraries(root: &Path) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    let mut add = |p: PathBuf| {
        if seen.insert(p.to_string_lossy().to_lowercase()) {
            out.push(p);
        }
    };
    add(root.to_path_buf());
    if let Some(v) = read_vdf(&root.join("steamapps").join("libraryfolders.vdf")) {
        if let Some(Vdf::Obj(libs)) = v.get(&["libraryfolders"]) {
            for (_, lib) in libs {
                if let Some(p) = lib.str(&["path"]) {
                    add(PathBuf::from(p.replace("\\\\", "\\")));
                }
            }
        }
    }
    out
}

#[derive(Debug, Clone)]
pub struct SteamGame {
    pub app_id: String,
    pub name: String,
    pub cover: String,
    pub hero: String,
    pub logo: String,
}

fn excluded(name: &str) -> bool {
    let n = name.to_lowercase();
    ["redistributable", "proton", "steam linux runtime", "steamvr", "steamworks", "soundtrack", "dedicated server"]
        .iter()
        .any(|w| n.contains(w))
        || n.split(|c: char| !c.is_alphanumeric()).any(|w| w == "sdk")
}

/// Every installed Steam game (tools, runtimes and soundtracks left out), sorted by name.
pub fn scan() -> Result<Vec<SteamGame>, String> {
    let root = root().ok_or("Steam не найден на этом компьютере")?;
    let mut seen = HashSet::new();
    let mut games = Vec::new();
    for lib in libraries(&root) {
        let Ok(dir) = std::fs::read_dir(lib.join("steamapps")) else { continue };
        for entry in dir.flatten() {
            let file = entry.file_name().to_string_lossy().to_string();
            if !(file.starts_with("appmanifest_") && file.ends_with(".acf")) {
                continue;
            }
            let Some(v) = read_vdf(&entry.path()) else { continue };
            let (Some(id), Some(name)) = (v.str(&["AppState", "appid"]), v.str(&["AppState", "name"])) else {
                continue;
            };
            if id == "228980" || excluded(name) || !seen.insert(id.to_string()) {
                continue;
            }
            games.push(SteamGame {
                app_id: id.into(),
                name: name.into(),
                cover: art(&root, id, "library_600x900.jpg"),
                hero: art(&root, id, "library_hero.jpg"),
                logo: art(&root, id, "logo.png"),
            });
        }
    }
    games.sort_by_key(|g| g.name.to_lowercase());
    Ok(games)
}

/// Path to a piece of art in Steam's own cache, or the CDN URL when the client hasn't cached it.
/// Newer Steam clients keep art in `librarycache/<appid>/<hash>/file`, so look one level deeper too.
pub fn art(root: &Path, id: &str, file: &str) -> String {
    local_art(root, id, file).unwrap_or_else(|| format!("{CDN}/{id}/{file}"))
}

pub fn local_art(root: &Path, id: &str, file: &str) -> Option<String> {
    let cache = root.join("appcache").join("librarycache");
    let flat = cache.join(format!("{id}_{file}"));
    if flat.exists() {
        return Some(flat.to_string_lossy().into_owned());
    }
    let dir = cache.join(id);
    let direct = dir.join(file);
    if direct.exists() {
        return Some(direct.to_string_lossy().into_owned());
    }
    std::fs::read_dir(&dir).ok()?.flatten().find_map(|e| {
        let p = e.path().join(file);
        p.exists().then(|| p.to_string_lossy().into_owned())
    })
}

/// Art candidates for a Steam game, best first: the local cache, then the CDN, then smaller CDN art.
pub fn cover_candidates(root: Option<&Path>, id: &str) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(root) = root {
        for f in ["library_600x900.jpg", "library_600x900_2x.jpg"] {
            if let Some(p) = local_art(&root, id, f) {
                out.push(p);
            }
        }
    }
    for f in ["library_600x900.jpg", "header.jpg", "capsule_616x353.jpg"] {
        out.push(format!("{CDN}/{id}/{f}"));
    }
    out
}

pub fn hero_candidates(root: Option<&Path>, id: &str) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(root) = root {
        if let Some(p) = local_art(&root, id, "library_hero.jpg") {
            out.push(p);
        }
    }
    out.push(format!("{CDN}/{id}/library_hero.jpg"));
    out.push(format!("{CDN}/{id}/header.jpg"));
    out
}

pub fn logo_candidates(root: Option<&Path>, id: &str) -> Vec<String> {
    let mut out = Vec::new();
    if let Some(root) = root {
        if let Some(p) = local_art(&root, id, "logo.png") {
            out.push(p);
        }
    }
    out.push(format!("{CDN}/{id}/logo.png"));
    out
}

// ---------- playtime recorded by the Steam client ----------

#[derive(Debug, Clone, Copy, Default)]
pub struct LocalStat {
    /// seconds
    pub playtime: f64,
    /// unix milliseconds
    pub last_played: f64,
}

/// Per-app playtime of the Steam account used last (the userdata folder with the freshest config).
pub fn local_stats() -> (Option<String>, HashMap<String, LocalStat>) {
    let mut apps = HashMap::new();
    let Some(root) = root() else { return (None, apps) };
    let best = std::fs::read_dir(root.join("userdata")).ok().and_then(|dir| {
        dir.flatten()
            .filter_map(|e| {
                let name = e.file_name().to_string_lossy().to_string();
                if name == "0" || !name.chars().all(|c| c.is_ascii_digit()) {
                    return None;
                }
                let f = e.path().join("config").join("localconfig.vdf");
                let t = std::fs::metadata(&f).and_then(|m| m.modified()).ok()?;
                Some((t, name, f))
            })
            .max_by_key(|(t, _, _)| *t)
    });
    let Some((_, account, file)) = best else { return (None, apps) };
    let steam_id = account.parse::<u64>().ok().map(|a| (a + STEAMID_BASE).to_string());
    if let Some(v) = read_vdf(&file) {
        if let Some(list) = v.get(&["UserLocalConfigStore", "Software", "Valve", "Steam", "apps"]) {
            for (id, a) in list.entries() {
                let num = |k: &str| a.str(&[k]).and_then(|s| s.parse::<f64>().ok()).unwrap_or(0.0);
                let st = LocalStat { playtime: num("Playtime") * 60.0, last_played: num("LastPlayed") * 1000.0 };
                if st.playtime > 0.0 || st.last_played > 0.0 {
                    apps.insert(id.clone(), st);
                }
            }
        }
    }
    (steam_id, apps)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_vdf() {
        let v = Vdf::parse(
            r#"
            "AppState"
            {
                "appid"     "730"
                // comment
                "name"      "Counter-Strike 2"
                "UserConfig" { "language" "russian" }
                "path"  "D:\\Games"
            }"#,
        );
        assert_eq!(v.str(&["appstate", "APPID"]), Some("730"));
        assert_eq!(v.str(&["AppState", "name"]), Some("Counter-Strike 2"));
        assert_eq!(v.str(&["AppState", "UserConfig", "language"]), Some("russian"));
        assert_eq!(v.str(&["AppState", "path"]), Some("D:\\Games"));
        assert!(v.get(&["AppState", "missing"]).is_none());
    }

    #[test]
    fn excludes_tools() {
        assert!(excluded("Steamworks Common Redistributables"));
        assert!(excluded("Some Game SDK"));
        assert!(!excluded("Hades"));
        assert!(!excluded("Sdkfoo Adventures"));
    }
}

#[cfg(test)]
mod live {
    /// Runs against this PC's real Steam install: `cargo test -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn scan_real_install() {
        let games = super::scan().expect("scan");
        println!("found {} games", games.len());
        for g in &games {
            println!("{} | {} | cover: {}", g.app_id, g.name, g.cover);
        }
        let (id, stats) = super::local_stats();
        println!("steam id known: {}, apps with playtime: {}", id.is_some(), stats.len());
        assert!(!games.is_empty());
    }
}

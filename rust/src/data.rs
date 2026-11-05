//! The library file shared with the Electron build: %APPDATA%\OTIUM\library.json.
//! Unknown fields are kept as-is, so saving from here never loses data the old version wrote.

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::path::PathBuf;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Game {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub args: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steam_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cover: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hero: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logo: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none", serialize_with = "whole_number")]
    pub order: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none", serialize_with = "whole_number")]
    pub added: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none", serialize_with = "whole_number")]
    pub playtime: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none", serialize_with = "whole_number")]
    pub last_played: Option<f64>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// JavaScript wrote whole numbers without ".0"; keep them that way.
fn whole_number<S: serde::Serializer>(v: &Option<f64>, s: S) -> Result<S::Ok, S::Error> {
    match v {
        Some(x) if x.fract() == 0.0 && x.abs() < 9.0e15 => s.serialize_i64(*x as i64),
        Some(x) => s.serialize_f64(*x),
        None => s.serialize_none(),
    }
}

impl Game {
    pub fn steam_app_id(&self) -> Option<String> {
        if let Some(id) = self.steam_id.as_ref().filter(|s| !s.is_empty()) {
            return Some(id.clone());
        }
        let p = self.path.to_ascii_lowercase();
        for prefix in ["steam://rungameid/", "steam://run/"] {
            if let Some(rest) = p.strip_prefix(prefix) {
                let id: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
                if !id.is_empty() {
                    return Some(id);
                }
            }
        }
        None
    }

    pub fn category(&self) -> &str {
        self.extra.get("category").and_then(Value::as_str).unwrap_or("game")
    }

    pub fn is_app(&self) -> bool {
        self.category() == "app"
    }

    /// A real file on disk (not a steam:// or https:// link).
    pub fn is_file(&self) -> bool {
        !self.path.contains("://") && std::path::Path::new(&self.path).is_absolute()
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Library {
    #[serde(default)]
    pub games: Vec<Game>,
    #[serde(default)]
    pub settings: Map<String, Value>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Library {
    /// `OTIUM_DATA` points the app at another folder (for testing on a copy of the library).
    pub fn data_dir() -> PathBuf {
        if let Some(dir) = std::env::var_os("OTIUM_DATA") {
            return PathBuf::from(dir);
        }
        dirs::config_dir().unwrap_or_else(|| PathBuf::from(".")).join("OTIUM")
    }

    pub fn file() -> PathBuf {
        Self::data_dir().join("library.json")
    }

    /// Covers and backgrounds the user picked live here, like in the Electron build.
    pub fn media_dir() -> PathBuf {
        Self::data_dir().join("media")
    }

    /// A fresh file name in the media folder: `<ms>_<random>.<ext>`.
    pub fn media_file(prefix: &str, ext: &str) -> PathBuf {
        let rand: String = uuid::Uuid::new_v4().simple().to_string().chars().take(6).collect();
        Self::media_dir().join(format!("{prefix}{}_{rand}.{ext}", now_ms() as u64))
    }

    /// Copies a picked file into the media folder and returns its `file:///` URL.
    pub fn import_media(src: &std::path::Path) -> std::io::Result<String> {
        std::fs::create_dir_all(Self::media_dir())?;
        let ext = src.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_else(|| "jpg".into());
        let dest = Self::media_file("", &ext);
        std::fs::copy(src, &dest)?;
        Ok(file_url(&dest.to_string_lossy()))
    }

    /// The exe's icon as a cover image (`file:///` URL), or None when it has none.
    pub fn exe_icon(exe: &str) -> Option<String> {
        let p = std::path::Path::new(exe);
        if !exe.to_lowercase().ends_with(".exe") || !p.is_file() {
            return None;
        }
        let dest = Self::media_file("icon_", "png");
        crate::win::exe_icon_png(p, &dest).then(|| file_url(&dest.to_string_lossy()))
    }

    pub fn load() -> Self {
        let Ok(text) = std::fs::read_to_string(Self::file()) else {
            return Self::default();
        };
        let text = text.trim_start_matches('\u{feff}');
        serde_json::from_str(text).unwrap_or_else(|e| {
            eprintln!("library.json: {e}");
            Self::default()
        })
    }

    /// Atomic write: a crash mid-save never leaves a half-written library behind.
    #[allow(dead_code)]
    pub fn save(&self) -> std::io::Result<()> {
        let file = Self::file();
        std::fs::create_dir_all(Self::data_dir())?;
        let tmp = file.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(tmp, file)
    }

    pub fn setting_str(&self, key: &str) -> &str {
        self.settings.get(key).and_then(Value::as_str).unwrap_or("")
    }

    pub fn setting_bool(&self, key: &str, default: bool) -> bool {
        self.settings.get(key).and_then(Value::as_bool).unwrap_or(default)
    }

    /// Games in the order the home row shows them.
    pub fn sorted_games(&self) -> Vec<Game> {
        let mut games = self.games.clone();
        match self.setting_str("sortMode") {
            "name" | "alpha" => games.sort_by_key(|g| g.name.to_lowercase()),
            // like the Electron build, no setting means "recently played first"
            "recent" | "" => games.sort_by(|a, b| {
                b.last_played.unwrap_or(0.0).total_cmp(&a.last_played.unwrap_or(0.0))
            }),
            _ => games.sort_by(|a, b| {
                let ka = a.order.unwrap_or(f64::MAX);
                let kb = b.order.unwrap_or(f64::MAX);
                ka.total_cmp(&kb)
                    .then(a.added.unwrap_or(0.0).total_cmp(&b.added.unwrap_or(0.0)))
            }),
        }
        games
    }
}

/// Turns a stored image reference (URL or local path) into something the image loaders accept.
/// The Electron build stored local files as `file:///D:/x%20y.jpg`; egui's file loader wants
/// `file://D:/x y.jpg`.
pub fn image_uri(src: &str) -> Option<String> {
    let s = src.trim();
    if s.is_empty() {
        return None;
    }
    if s.starts_with("http://") || s.starts_with("https://") {
        return Some(s.to_string());
    }
    let path = match s.strip_prefix("file://") {
        Some(rest) => percent_decode(rest.trim_start_matches('/')),
        None => s.to_string(),
    };
    Some(format!("file://{}", path.replace('\\', "/")))
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Some(v) = s.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Local paths are stored as `file:///` URLs, the way the Electron build stores them.
pub fn file_url(src: &str) -> String {
    if src.contains("://") {
        return src.to_string();
    }
    let mut out = String::from("file:///");
    for c in src.replace('\\', "/").chars() {
        match c {
            ' ' => out.push_str("%20"),
            '%' => out.push_str("%25"),
            '#' => out.push_str("%23"),
            _ => out.push(c),
        }
    }
    out
}

fn now_ms() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as f64)
        .unwrap_or(0.0)
}

impl Game {
    pub fn new(name: &str, path: &str) -> Self {
        let mut extra = Map::new();
        extra.insert("category".into(), Value::from("game"));
        Game {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.into(),
            path: path.into(),
            args: Some(String::new()),
            added: Some(now_ms()),
            extra,
            ..Default::default()
        }
    }

    pub fn from_steam(g: &crate::steam::SteamGame, added: f64) -> Self {
        let mut game = Game::new(&g.name, &format!("steam://rungameid/{}", g.app_id));
        game.steam_id = Some(g.app_id.clone());
        game.cover = Some(file_url(&g.cover));
        game.hero = Some(file_url(&g.hero));
        game.logo = Some(file_url(&g.logo));
        game.added = Some(added);
        game
    }

    /// A background the user picked for this game themselves.
    pub fn custom_background(&self) -> Option<&str> {
        self.extra.get("background").and_then(Value::as_str).filter(|s| !s.is_empty())
    }

    /// The Store's id for this game, when it was installed from there.
    pub fn store_id(&self) -> Option<&str> {
        self.extra.get("storeId").and_then(Value::as_str)
    }

    pub fn set_store_id(&mut self, id: &str) {
        self.extra.insert("storeId".into(), Value::from(id));
    }

    pub fn set_background(&mut self, url: &str) {
        self.extra.insert("background".into(), Value::from(url));
    }
}

#[cfg(test)]
mod tests {
    use super::image_uri;

    #[test]
    fn uris() {
        assert_eq!(image_uri("D:\\Steam\\a b.jpg").unwrap(), "file://D:/Steam/a b.jpg");
        assert_eq!(image_uri("file:///D:/Steam/a%20b.jpg").unwrap(), "file://D:/Steam/a b.jpg");
        assert_eq!(image_uri("https://x/y.jpg").unwrap(), "https://x/y.jpg");
        assert_eq!(image_uri("file:///C:/100%").unwrap(), "file://C:/100%");
        assert!(image_uri("  ").is_none());
        let u = super::file_url("d:\\Steam\\my game\\a.jpg");
        assert_eq!(u, "file:///d:/Steam/my%20game/a.jpg");
        assert_eq!(image_uri(&u).unwrap(), "file://d:/Steam/my game/a.jpg");
    }
}

#[cfg(test)]
mod live {
    /// The exe-icon cover: notepad's icon comes out as a 256×256 PNG with something drawn in it.
    #[test]
    #[ignore]
    fn exe_icon() {
        let out = std::env::temp_dir().join("otium_icon_test.png");
        let exe = std::path::Path::new(r"C:\Windows\System32\notepad.exe");
        assert!(crate::win::exe_icon_png(exe, &out), "no icon");
        let img = image::open(&out).unwrap().to_rgba8();
        assert_eq!(img.dimensions(), (256, 256));
        let opaque = img.pixels().filter(|p| p[3] > 0).count();
        println!("icon: {opaque} visible pixels");
        assert!(opaque > 1000);
        let _ = std::fs::remove_file(out);
    }

    /// Load + serialize the real library in memory and check nothing is lost (nothing is written).
    #[test]
    #[ignore]
    fn round_trip_real_library() {
        let text = std::fs::read_to_string(super::Library::file()).expect("library.json");
        let original: serde_json::Value = serde_json::from_str(text.trim_start_matches('\u{feff}')).unwrap();
        let lib: super::Library = serde_json::from_value(original.clone()).unwrap();
        let back = serde_json::to_value(&lib).unwrap();
        let mut diffs = Vec::new();
        diff("", &original, &back, &mut diffs);
        assert!(diffs.is_empty(), "round trip changed: {diffs:?}");
        println!("round trip ok: {} games, {} top-level keys", lib.games.len(), original.as_object().unwrap().len());
    }

    /// Paths where two JSON trees differ (values are not printed: the library holds private data).
    fn diff(path: &str, a: &serde_json::Value, b: &serde_json::Value, out: &mut Vec<String>) {
        use serde_json::Value::*;
        match (a, b) {
            (Object(x), Object(y)) => {
                for k in x.keys().chain(y.keys().filter(|k| !x.contains_key(*k))) {
                    match (x.get(k), y.get(k)) {
                        (Some(va), Some(vb)) => diff(&format!("{path}.{k}"), va, vb, out),
                        _ => out.push(format!("{path}.{k} missing on one side")),
                    }
                }
            }
            (Array(x), Array(y)) if x.len() == y.len() => {
                for (i, (va, vb)) in x.iter().zip(y).enumerate() {
                    diff(&format!("{path}[{i}]"), va, vb, out);
                }
            }
            _ if a == b => {}
            _ => out.push(format!("{path} differs")),
        }
    }
}

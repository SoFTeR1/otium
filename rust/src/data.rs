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
            "name" => games.sort_by_key(|g| g.name.to_lowercase()),
            "recent" => games.sort_by(|a, b| {
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

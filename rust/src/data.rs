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
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub args: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub steam_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cover: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hero: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub logo: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub order: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub added: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub playtime: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_played: Option<f64>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
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
    pub fn data_dir() -> PathBuf {
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
pub fn image_uri(src: &str) -> Option<String> {
    let s = src.trim();
    if s.is_empty() {
        return None;
    }
    if s.starts_with("http://") || s.starts_with("https://") || s.starts_with("file://") {
        return Some(s.to_string());
    }
    Some(format!("file://{}", s.replace('\\', "/")))
}

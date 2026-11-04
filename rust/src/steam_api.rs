//! Steam Web API: news and trophies. Blocking calls, made from worker
//! threads; results are cached for a while like the Electron build did.

use regex::Regex;
use serde_json::Value;
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

const CLAN_IMG: &str = "https://clan.cloudflare.steamstatic.com/images";

static CACHE: LazyLock<Mutex<HashMap<String, (Instant, Value)>>> = LazyLock::new(Default::default);

fn cached(key: &str, ttl: Duration, f: impl FnOnce() -> Option<Value>) -> Option<Value> {
    if let Some((t, v)) = CACHE.lock().unwrap().get(key) {
        if t.elapsed() < ttl {
            return Some(v.clone());
        }
    }
    let v = f()?;
    CACHE.lock().unwrap().insert(key.to_string(), (Instant::now(), v.clone()));
    Some(v)
}

pub fn forget(prefix: &str) {
    CACHE.lock().unwrap().retain(|k, _| !k.starts_with(prefix));
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new().timeout(Duration::from_secs(15)).user_agent("OTIUM").build()
}

/// GET returning (status, json). Status 0 = network error; json is Null for non-JSON bodies.
fn probe(url: &str) -> (u16, Value) {
    match agent().get(url).set("Accept-Language", "ru").call() {
        Ok(r) => (r.status(), r.into_json().unwrap_or(Value::Null)),
        Err(ureq::Error::Status(code, r)) => (code, r.into_json().unwrap_or(Value::Null)),
        Err(_) => (0, Value::Null),
    }
}

fn json(url: &str) -> Option<Value> {
    match probe(url) {
        (200, v) if !v.is_null() => Some(v),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApiError {
    BadKey,
    Private,
    Network,
    Other,
    /// the game has no trophies
    None,
}

// ---------- news ----------

#[derive(Debug, Clone)]
pub struct News {
    pub title: String,
    pub url: String,
    /// unix ms
    pub date: f64,
    pub feed: String,
    pub image: Option<String>,
    pub text: String,
}

fn news_image(contents: &str) -> Option<String> {
    static RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?i)\{STEAM_CLAN_IMAGE\}[^\s\[\]"'<>]+|https?://[^\s\[\]"'<>]+\.(?:png|jpe?g|gif|webp)"#).unwrap()
    });
    RE.find(contents).map(|m| m.as_str().replace("{STEAM_CLAN_IMAGE}", CLAN_IMG))
}

/// Steam's BBCode / HTML news body as plain text with paragraphs and bullets kept.
pub fn news_text(contents: &str) -> String {
    static RULES: LazyLock<Vec<(Regex, &'static str)>> = LazyLock::new(|| {
        [
            (r"(?i)\[img[^\]]*\][^\[]*\[/img\]|\[img[^\]]*\]", ""),
            (r"(?i)\[previewyoutube[^\]]*\][^\[]*\[/previewyoutube\]", ""),
            (r"\[\*\]", "\n• "),
            (r"(?i)\[(?:/?p|br|/h\d|/list|hr/?)\]", "\n"),
            (r"(?i)\[h\d\]", "\n\n"),
            (r"\[[^\]]+\]", ""),
            (r"(?i)<br\s*/?>|</p>", "\n"),
            (r"<[^>]+>", ""),
            (r"\{STEAM_CLAN_IMAGE\}\S*", ""),
        ]
        .into_iter()
        .map(|(p, r)| (Regex::new(p).unwrap(), r))
        .collect()
    });
    static SPACES: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"[ \t]+\n").unwrap());
    static BLANKS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\n{3,}").unwrap());
    let mut s = contents.to_string();
    for (re, rep) in RULES.iter() {
        s = re.replace_all(&s, *rep).into_owned();
    }
    for (from, to) in [("&nbsp;", " "), ("&amp;", "&"), ("&quot;", "\""), ("&#39;", "'"), ("&lt;", "<"), ("&gt;", ">")] {
        s = s.replace(from, to);
    }
    s = SPACES.replace_all(&s, "\n").into_owned();
    s = BLANKS.replace_all(&s, "\n\n").into_owned();
    s.trim().chars().take(6000).collect()
}

fn fetch_news(appid: &str, feeds: Option<&str>) -> Option<Vec<News>> {
    let feeds = feeds.map(|f| format!("&feeds={f}")).unwrap_or_default();
    let v = json(&format!(
        "https://api.steampowered.com/ISteamNews/GetNewsForApp/v2/?appid={appid}&count=8&maxlength=0&format=json{feeds}"
    ))?;
    let items = v["appnews"]["newsitems"].as_array()?;
    Some(
        items
            .iter()
            .map(|n| {
                let contents = n["contents"].as_str().unwrap_or("");
                News {
                    title: n["title"].as_str().unwrap_or("").to_string(),
                    url: n["url"].as_str().unwrap_or("").to_string(),
                    date: n["date"].as_f64().unwrap_or(0.0) * 1000.0,
                    feed: n["feedlabel"].as_str().unwrap_or("").to_string(),
                    image: news_image(contents),
                    text: news_text(contents),
                }
            })
            .collect(),
    )
}

/// Official announcements first (they carry images); all feeds when there are none.
pub fn news(appid: &str) -> Vec<News> {
    static NEWS: LazyLock<Mutex<HashMap<String, (Instant, Vec<News>)>>> = LazyLock::new(Default::default);
    if let Some((t, v)) = NEWS.lock().unwrap().get(appid) {
        if t.elapsed() < Duration::from_secs(30 * 60) {
            return v.clone();
        }
    }
    let mut items = fetch_news(appid, Some("steam_community_announcements")).unwrap_or_default();
    if items.is_empty() {
        items = fetch_news(appid, None).unwrap_or_default();
    }
    NEWS.lock().unwrap().insert(appid.to_string(), (Instant::now(), items.clone()));
    items
}

// ---------- trophies ----------

#[derive(Debug, Clone)]
pub struct Achievement {
    pub name: String,
    pub desc: String,
    pub icon: Option<String>,
    pub achieved: bool,
    /// unix ms
    pub time: f64,
    pub percent: Option<f64>,
}

#[derive(Debug, Clone)]
pub struct Trophies {
    pub items: Vec<Achievement>,
    pub done: usize,
}

pub fn achievements(appid: &str, key: &str, steam_id: &str, fresh: bool) -> Result<Trophies, ApiError> {
    if fresh {
        forget(&format!("ach{appid}:"));
    }
    let ck = format!("ach{appid}:{steam_id}");
    let ttl = Duration::from_secs(5 * 60);
    let player = format!(
        "https://api.steampowered.com/ISteamUserStats/GetPlayerAchievements/v1/?appid={appid}&key={key}&steamid={steam_id}&l=russian"
    );
    let (status, p) = match CACHE.lock().unwrap().get(&ck) {
        Some((t, v)) if t.elapsed() < ttl => (200, v.clone()),
        _ => probe(&player),
    };
    if (status == 401 || status == 403) && p.is_null() {
        return Err(ApiError::BadKey);
    }
    if status == 0 {
        return Err(ApiError::Network);
    }
    let Some(list) = p["playerstats"]["achievements"].as_array() else {
        let msg = p["playerstats"]["error"].as_str().unwrap_or("").to_lowercase();
        return Err(if msg.contains("not public") || msg.contains("private") {
            ApiError::Private
        } else if msg.contains("no stats") || msg.contains("not found") || status == 400 {
            ApiError::None
        } else {
            ApiError::Other
        });
    };
    CACHE.lock().unwrap().insert(ck, (Instant::now(), p.clone()));

    let schema = cached(&format!("schema{appid}"), Duration::from_secs(24 * 3600), || {
        json(&format!("https://api.steampowered.com/ISteamUserStats/GetSchemaForGame/v2/?appid={appid}&key={key}&l=russian"))
    })
    .unwrap_or(Value::Null);
    let global = cached(&format!("global{appid}"), Duration::from_secs(6 * 3600), || {
        json(&format!(
            "https://api.steampowered.com/ISteamUserStats/GetGlobalAchievementPercentagesForApp/v2/?gameid={appid}"
        ))
    })
    .unwrap_or(Value::Null);

    let meta: HashMap<&str, &Value> = schema["game"]["availableGameStats"]["achievements"]
        .as_array()
        .map(|a| a.iter().filter_map(|x| Some((x["name"].as_str()?, x))).collect())
        .unwrap_or_default();
    let pct: HashMap<&str, f64> = global["achievementpercentages"]["achievements"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|x| {
                    let p = x["percent"].as_f64().or_else(|| x["percent"].as_str()?.parse().ok())?;
                    Some((x["name"].as_str()?, p))
                })
                .collect()
        })
        .unwrap_or_default();

    let items: Vec<Achievement> = list
        .iter()
        .map(|a| {
            let api = a["apiname"].as_str().unwrap_or("");
            let m = meta.get(api);
            let achieved = a["achieved"].as_i64().unwrap_or(0) != 0;
            let s = |k: &str| m.and_then(|m| m[k].as_str()).map(str::to_string);
            Achievement {
                name: s("displayName").or_else(|| a["name"].as_str().map(str::to_string)).unwrap_or_else(|| api.to_string()),
                desc: s("description").or_else(|| a["description"].as_str().map(str::to_string)).unwrap_or_default(),
                icon: if achieved { s("icon") } else { s("icongray") },
                achieved,
                time: a["unlocktime"].as_f64().unwrap_or(0.0) * 1000.0,
                percent: pct.get(api).copied(),
            }
        })
        .collect();
    if items.is_empty() {
        return Err(ApiError::None);
    }
    let done = items.iter().filter(|a| a.achieved).count();
    Ok(Trophies { items, done })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cleans_news() {
        let t = news_text("[h1]Patch[/h1][p]Fixed &amp; improved[/p][list][*]one[*]two[/list][img]{STEAM_CLAN_IMAGE}/a.png[/img]<br>end");
        assert!(t.starts_with("Patch"));
        assert!(t.contains("Fixed & improved"));
        assert!(t.contains("• one"));
        assert!(!t.contains("STEAM_CLAN_IMAGE"));
        assert!(t.ends_with("end"));
    }

    #[test]
    fn finds_image() {
        assert_eq!(news_image("x {STEAM_CLAN_IMAGE}/3/abc.png y").unwrap(), format!("{CLAN_IMG}/3/abc.png"));
        assert_eq!(news_image("see https://a.b/c.jpg").unwrap(), "https://a.b/c.jpg");
        assert!(news_image("nothing").is_none());
    }
}

#[cfg(test)]
mod live {
    /// Real network call: `cargo test -- --ignored live_news --nocapture`.
    #[test]
    #[ignore]
    fn live_news() {
        let items = super::news("1808500");
        println!("news: {}", items.len());
        for n in items.iter().take(3) {
            println!("- {} | img: {} | text chars: {}", n.title, n.image.is_some(), n.text.chars().count());
        }
        assert!(!items.is_empty());
    }
}

#[cfg(test)]
mod live_trophies {
    /// Uses the key and Steam id from the real library (never printed).
    #[test]
    #[ignore]
    fn live_trophies() {
        let lib = crate::data::Library::load();
        let key = lib.setting_str("steamApiKey").to_string();
        let user = lib.setting_str("steamId").to_string();
        for app in ["1808500", "2399420", "4080220"] {
            match super::achievements(app, &key, &user, true) {
                Ok(t) => println!("{app}: {} / {}", t.done, t.items.len()),
                Err(e) => println!("{app}: {e:?}"),
            }
        }
    }
}

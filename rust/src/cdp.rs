//! A minimal Chrome DevTools Protocol client (blocking, one page at a time): enough to make a
//! service window look like a TV to the site in it.
//!
//! The browser is started with `--remote-debugging-port=0`; it picks a free local port and writes
//! it to `DevToolsActivePort` in its profile folder.

use serde_json::{json, Value};
use std::net::TcpStream;
use std::path::Path;
use std::time::{Duration, Instant};
use tungstenite::stream::MaybeTlsStream;
use tungstenite::{Message, WebSocket};

pub struct Session {
    ws: WebSocket<MaybeTlsStream<TcpStream>>,
    next: u64,
}

/// The port the browser listens on, once it has written it (it does so a moment after starting).
pub fn port(profile: &Path, timeout: Duration) -> Option<u16> {
    let file = profile.join("DevToolsActivePort");
    let at = Instant::now();
    while at.elapsed() < timeout {
        if let Ok(t) = std::fs::read_to_string(&file) {
            if let Some(p) = t.lines().next().and_then(|l| l.trim().parse().ok()) {
                return Some(p);
            }
        }
        std::thread::sleep(Duration::from_millis(150));
    }
    None
}

/// The websocket address of the browser's first page.
pub fn page(port: u16, timeout: Duration) -> Option<String> {
    let at = Instant::now();
    while at.elapsed() < timeout {
        let list: Option<Value> = ureq::get(&format!("http://127.0.0.1:{port}/json/list")).timeout(Duration::from_secs(2)).call().ok().and_then(|r| r.into_json().ok());
        let found = list.as_ref().and_then(Value::as_array).and_then(|l| l.iter().find(|t| t["type"] == "page")).and_then(|t| t["webSocketDebuggerUrl"].as_str());
        if let Some(u) = found {
            return Some(u.to_string());
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    None
}

impl Session {
    pub fn connect(url: &str) -> Result<Self, String> {
        let (ws, _) = tungstenite::connect(url).map_err(|e| format!("DevTools: {e}"))?;
        Ok(Self { ws, next: 1 })
    }

    /// Sends a command and waits for its answer (events in between are skipped).
    pub fn call(&mut self, method: &str, params: Value) -> Result<Value, String> {
        let id = self.next;
        self.next += 1;
        let msg = json!({ "id": id, "method": method, "params": params }).to_string();
        self.ws.send(Message::text(msg)).map_err(|e| format!("DevTools: {e}"))?;
        loop {
            let m = self.ws.read().map_err(|e| format!("DevTools: {e}"))?;
            let Message::Text(t) = m else { continue };
            let v: Value = serde_json::from_str(t.as_str()).unwrap_or_default();
            if v["id"] == id {
                if let Some(e) = v.get("error") {
                    return Err(format!("DevTools {method}: {e}"));
                }
                return Ok(v["result"].clone());
            }
        }
    }

    /// Value of a JavaScript expression in the page.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn eval(&mut self, expr: &str) -> Result<Value, String> {
        let r = self.call("Runtime.evaluate", json!({ "expression": expr, "returnByValue": true }))?;
        Ok(r["result"]["value"].clone())
    }

    /// Blocks until the browser goes away. Overrides last only while the session is attached.
    pub fn hold(mut self) {
        while self.ws.read().is_ok() {}
    }
}

/// Puts the page's window in fullscreen (works whatever window has the focus, unlike F11).
pub fn fullscreen(s: &mut Session) -> Result<(), String> {
    let w = s.call("Browser.getWindowForTarget", json!({}))?;
    s.call("Browser.setWindowBounds", json!({ "windowId": w["windowId"], "bounds": { "windowState": "fullscreen" } }))?;
    Ok(())
}

/// Makes the page present itself as `ua` (both the user agent string and the client hints a page
/// can read with `navigator.userAgentData`) and opens `url`.
pub fn pose_as(s: &mut Session, ua: &str, url: Option<&str>) -> Result<(), String> {
    s.call(
        "Emulation.setUserAgentOverride",
        json!({
            "userAgent": ua,
            "platform": "Linux",
            "userAgentMetadata": {
                "brands": [], "fullVersionList": [], "fullVersion": "",
                "platform": "Linux", "platformVersion": "", "architecture": "arm", "model": "", "mobile": false,
            },
        }),
    )?;
    if let Some(url) = url {
        s.call("Page.navigate", json!({ "url": url }))?;
    }
    Ok(())
}

#[cfg(test)]
mod live {
    use super::*;

    /// cargo test --release tv_agents -- --ignored --nocapture
    /// Which user agents get YouTube's TV interface in a headless Edge.
    #[test]
    #[ignore]
    fn tv_agents() {
        let exe = crate::services::browser().expect("no browser");
        let dir = std::env::temp_dir().join("otium-cdp-test");
        let _ = std::fs::remove_dir_all(&dir);
        let mut child = std::process::Command::new(exe)
            .args(["--headless=new", "--remote-debugging-port=0", "--no-first-run", "about:blank"])
            .arg(format!("--user-data-dir={}", dir.display()))
            .spawn()
            .unwrap();
        let port = port(&dir, Duration::from_secs(15)).expect("no port");
        let url = page(port, Duration::from_secs(10)).expect("no page");
        let mut s = Session::connect(&url).unwrap();
        for ua in crate::services::TV_AGENTS {
            pose_as(&mut s, ua, Some("https://www.youtube.com/tv")).unwrap();
            std::thread::sleep(Duration::from_secs(12));
            let v = s.eval("location.href + ' | ' + (document.querySelector('ytlr-app') ? 'TV' : 'web')").unwrap();
            println!("{v}  <=  {ua}");
        }
        let _ = child.kill();
        let _ = std::fs::remove_dir_all(&dir);
    }
}

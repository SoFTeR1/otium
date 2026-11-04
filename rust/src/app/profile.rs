//! Trophies earned while playing (announced when the game closes, remembered as `lastTrophy`),
//! public game art for the Discord status, and a friend's profile: their library and last trophy
//! as they publish it (Nostr kind 30078, `d` = "ps5l-library", shared with the Electron build).

use super::App;
use crate::draw::{self, bold, font, semibold, white};
use crate::input::Nav;
use crate::modal::Modal;
use crate::watch::Event;
use egui::{pos2, vec2, Align2, Color32, CornerRadius, Rect, Stroke, StrokeKind};
use serde_json::{json, Value};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const GREEN: Color32 = Color32::from_rgb(67, 181, 129);

fn ms(t: SystemTime) -> f64 {
    t.duration_since(UNIX_EPOCH).map(|d| d.as_millis() as f64).unwrap_or(0.0)
}

/// "12 ч", "35 мин"
fn hours(secs: f64) -> String {
    let m = (secs / 60.0).round() as i64;
    if m < 60 { format!("{} мин", m.max(0)) } else { format!("{} ч", m / 60) }
}

impl App {
    // ---------- trophies ----------

    /// A Steam game closed: Steam needs a few seconds to sync its achievements, then the fresh
    /// list says what was earned in this session (and up to six hours before, like Electron).
    pub(super) fn check_trophies_later(&self, appid: u32, started: SystemTime) {
        let (key, user) = (self.steam_key(), self.steam_user());
        if key.is_empty() || user.is_empty() {
            return;
        }
        let bus = self.bus.clone();
        let since = ms(started) - 6.0 * 3600e3;
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_secs(10));
            let res = crate::steam_api::achievements(&appid.to_string(), &key, &user, true);
            bus.send(Event::TrophyCheck { appid: appid.to_string(), since, res });
        });
    }

    pub(super) fn on_trophy_check(&mut self, appid: &str, since: f64, res: Result<crate::steam_api::Trophies, crate::steam_api::ApiError>) {
        let Ok(t) = res else { return };
        let items: Vec<_> = t.items.into_iter().filter(|a| a.time >= since).collect();
        let game = self.steam_game_name(appid);
        self.note_trophy(&game, appid, &items, true);
        // the trophies page shows the fresh list too
        self.trophies_changed(appid.parse().unwrap_or(0));
    }

    fn steam_game_name(&self, appid: &str) -> String {
        self.lib.games.iter().find(|g| g.steam_app_id().as_deref() == Some(appid)).map(|g| g.name.clone()).unwrap_or_else(|| "Steam".into())
    }

    /// Remembers the newest earned trophy if it's newer than `lastTrophy`; `announce` shows it.
    pub(super) fn note_trophy(&mut self, game: &str, appid: &str, items: &[crate::steam_api::Achievement], announce: bool) {
        let Some(newest) = items.iter().filter(|a| a.achieved && a.time > 0.0).max_by(|a, b| a.time.total_cmp(&b.time)) else { return };
        let last = self.lib.extra.get("lastTrophy").and_then(|t| t["time"].as_f64()).unwrap_or(0.0);
        if newest.time <= last {
            return;
        }
        let rec = json!({ "game": game, "appid": appid, "name": newest.name, "icon": newest.icon, "time": newest.time as i64 });
        self.lib.extra.insert("lastTrophy".into(), rec);
        self.save();
        self.publish_library();
        if !announce {
            return;
        }
        let got = items.iter().filter(|a| a.achieved && a.time > 0.0).count();
        let text = if got > 1 { format!("{} — {game} (и ещё {})", newest.name, got - 1) } else { format!("{} — {game}", newest.name) };
        if self.sounds_on() {
            crate::sound::play(crate::sound::Sfx::Launch);
        }
        if self.away() {
            self.overlay.toast_img("Трофей получен", &text, newest.icon.clone());
        } else {
            self.toast(&format!("🏆 Трофей получен: {text}"), false);
        }
    }

    // ---------- public art for Discord ----------

    /// Discord shows pictures only by URL: for a running game that has none (not on Steam, cover
    /// from disk) look one up once and keep it on the game as `publicArt`, like Electron does.
    pub(super) fn ensure_public_art(&mut self) {
        let Some(g) = self.games.iter().find(|g| self.running.contains(&g.id)) else { return };
        if g.steam_app_id().is_some() || g.cover.as_deref().is_some_and(|c| c.starts_with("https://")) {
            return;
        }
        if g.extra.get("publicArt").is_some_and(|pa| pa["for"].as_str() == Some(&g.name)) {
            return;
        }
        if !self.art_searched.insert(g.name.clone()) {
            return;
        }
        let (id, name, bus) = (g.id.clone(), g.name.clone(), self.bus.clone());
        std::thread::spawn(move || {
            let url = crate::steam_api::find_public_art(&name);
            bus.send(Event::PublicArt { id, name, url });
        });
    }

    pub(super) fn on_public_art(&mut self, id: &str, name: &str, url: String) {
        let rec = json!({ "for": name, "url": url });
        for g in self.lib.games.iter_mut().chain(self.games.iter_mut()) {
            if g.id == id && g.name == name {
                g.extra.insert("publicArt".into(), rec.clone());
            }
        }
        self.save();
        if !url.is_empty() {
            self.discord_soon = true;
        }
    }

    // ---------- a friend's profile ----------

    pub(super) fn open_friend_profile(&mut self, pk: &str) {
        self.social.net.send(crate::nostr::Cmd::FetchLibrary(pk.to_string()));
        self.open(Modal::Friend { pk: pk.to_string(), lib: None, scroll: 0.0 });
    }

    pub(super) fn on_library(&mut self, pk: &str, content: Option<String>) {
        if let Some(Modal::Friend { pk: p, lib, .. }) = self.modal.as_mut() {
            if p == pk {
                *lib = Some(content.and_then(|c| serde_json::from_str::<Value>(&c).ok()));
            }
        }
    }

    pub(super) fn friend_profile_nav(&mut self, nav: Nav) {
        let Some(Modal::Friend { pk, scroll, .. }) = self.modal.as_mut() else { return };
        let pk = pk.clone();
        match nav {
            Nav::Up => *scroll = (*scroll - 240.0).max(0.0),
            Nav::Down => *scroll += 240.0,
            Nav::Back => self.modal = None,
            Nav::Ok => {
                self.modal = None;
                self.open_friend_chat(&pk);
            }
            Nav::Options => {
                self.modal = None;
                self.start_call(&pk);
            }
            _ => {}
        }
    }

    pub(super) fn paint_friend_profile(&mut self, ui: &egui::Ui, screen: Rect, s: f32, a: f32) {
        let Some(Modal::Friend { pk, lib, scroll }) = &self.modal else { return };
        let (pk, lib, mut sc) = (pk.clone(), lib.clone(), *scroll);
        let name = self.account().map(|acc| self.display_name(&acc, &pk)).unwrap_or_default();
        let (online, presence, in_game) = self.presence_of(&pk);
        let p = ui.painter();

        let w = (1400.0 * s).min(screen.width() - 80.0 * s);
        let h = (880.0 * s).min(screen.height() - 120.0 * s);
        let panel = Rect::from_center_size(screen.center() - vec2(0.0, 20.0 * s) + vec2(0.0, (1.0 - a) * 30.0 * s), vec2(w, h));
        let radius = CornerRadius::same((24.0 * s) as u8);
        draw::glow(p, panel, 24.0 * s, Color32::BLACK, 40.0 * s, 0.5 * a);
        p.rect_filled(panel, radius, Color32::from_rgba_unmultiplied(18, 21, 30, (250.0 * a) as u8));
        p.rect_stroke(panel, radius, Stroke::new(1.0_f32, white(0.08 * a)), StrokeKind::Inside);

        // header: avatar, name, since and code, what they're doing, stats
        let av = pos2(panel.left() + 84.0 * s, panel.top() + 86.0 * s);
        p.circle_filled(av, 48.0 * s, draw::name_color(&pk).gamma_multiply(a));
        p.text(av, Align2::CENTER_CENTER, name.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default(), bold(40.0 * s), white(a));
        let tx = av.x + 72.0 * s;
        p.text(pos2(tx, av.y - 30.0 * s), Align2::LEFT_CENTER, &name, bold(34.0 * s), white(a));
        let since = lib.as_ref().and_then(|l| l.as_ref()).and_then(|l| l["since"].as_f64()).filter(|t| *t > 1.5e12);
        let line2 = match since {
            Some(t) => format!("В OTIUM с {} · код {}", super::fmt_date(t), crate::nostr::short_code(&pk)),
            None => format!("Игрок OTIUM · код {}", crate::nostr::short_code(&pk)),
        };
        p.text(pos2(tx, av.y + 6.0 * s), Align2::LEFT_CENTER, line2, font(18.0 * s), white(0.6 * a));
        let dot = if in_game { GREEN } else if online { Color32::from_rgb(120, 200, 255) } else { white(0.35) };
        p.circle_filled(pos2(tx + 6.0 * s, av.y + 36.0 * s), 5.0 * s, dot.gamma_multiply(a));
        p.text(pos2(tx + 20.0 * s, av.y + 36.0 * s), Align2::LEFT_CENTER, presence, font(18.0 * s), if in_game { GREEN.gamma_multiply(a) } else { white(0.75 * a) });

        let games: Vec<Value> = lib.as_ref().and_then(|l| l.as_ref()).and_then(|l| l["games"].as_array().cloned()).unwrap_or_default();
        if lib.as_ref().is_some_and(|l| l.is_some()) {
            let total: f64 = games.iter().filter_map(|g| g["p"].as_f64()).sum();
            let mut x = panel.right() - 40.0 * s;
            for (n, label) in [(hours(total), "наиграно"), (games.len().to_string(), "игр")] {
                let chip = Rect::from_min_size(pos2(x - 150.0 * s, av.y - 40.0 * s), vec2(150.0 * s, 80.0 * s));
                p.rect_filled(chip, CornerRadius::same((14.0 * s) as u8), white(0.06 * a));
                p.text(pos2(chip.center().x, chip.top() + 30.0 * s), Align2::CENTER_CENTER, n, bold(26.0 * s), white(a));
                p.text(pos2(chip.center().x, chip.bottom() - 18.0 * s), Align2::CENTER_CENTER, label, font(15.0 * s), white(0.55 * a));
                x = chip.left() - 12.0 * s;
            }
        }

        // body: last trophy, then the games, scrolling under the header
        let body = Rect::from_min_max(pos2(panel.left() + 40.0 * s, panel.top() + 176.0 * s), pos2(panel.right() - 40.0 * s, panel.bottom() - 24.0 * s));
        p.line_segment([pos2(panel.left() + 32.0 * s, body.top() - 12.0 * s), pos2(panel.right() - 32.0 * s, body.top() - 12.0 * s)], Stroke::new(1.0_f32, white(0.06 * a)));
        let clip = p.with_clip_rect(body);
        let lib = match lib {
            None => {
                clip.text(body.center_top() + vec2(0.0, 60.0 * s), Align2::CENTER_CENTER, "Загружаем профиль…", font(20.0 * s), white(0.6 * a));
                return;
            }
            Some(None) => {
                let msg = "Друг ещё не открыл свою библиотеку — она появится, когда он обновит OTIUM";
                clip.text(body.center_top() + vec2(0.0, 60.0 * s), Align2::CENTER_CENTER, msg, font(20.0 * s), white(0.6 * a));
                return;
            }
            Some(Some(l)) => l,
        };
        let mut y = body.top() - sc;
        clip.text(pos2(body.left(), y), Align2::LEFT_TOP, "Последний трофей", semibold(20.0 * s), white(0.85 * a));
        y += 36.0 * s;
        let card = Rect::from_min_size(pos2(body.left(), y), vec2(620.0 * s, 96.0 * s));
        clip.rect_filled(card, CornerRadius::same((14.0 * s) as u8), white(0.05 * a));
        let t = &lib["trophy"];
        if t.is_object() {
            let ic = Rect::from_min_size(card.min + vec2(12.0 * s, 12.0 * s), vec2(72.0 * s, 72.0 * s));
            let shown = t["icon"].as_str().filter(|_| body.contains_rect(ic)).is_some_and(|u| draw::cover(ui, &[u.to_string()], ic, 10.0 * s, white(a), (0.5, 0.5)));
            if !shown {
                clip.text(ic.center(), Align2::CENTER_CENTER, "🏆", font(34.0 * s), white(a));
            }
            let x = ic.right() + 16.0 * s;
            clip.text(pos2(x, card.top() + 22.0 * s), Align2::LEFT_CENTER, t["game"].as_str().unwrap_or(""), font(15.0 * s), white(0.55 * a));
            clip.text(pos2(x, card.center().y), Align2::LEFT_CENTER, t["name"].as_str().unwrap_or(""), semibold(20.0 * s), white(a));
            let date = t["time"].as_f64().map(super::fmt_date).unwrap_or_default();
            clip.text(pos2(x, card.bottom() - 22.0 * s), Align2::LEFT_CENTER, date, font(15.0 * s), white(0.55 * a));
        } else {
            clip.text(card.left_center() + vec2(20.0 * s, 0.0), Align2::LEFT_CENTER, "Трофеев пока нет", font(18.0 * s), white(0.55 * a));
        }
        y = card.bottom() + 32.0 * s;

        clip.text(pos2(body.left(), y), Align2::LEFT_TOP, "Игры", semibold(20.0 * s), white(0.85 * a));
        y += 36.0 * s;
        let (cw, ch, gap) = (150.0 * s, 225.0 * s, 22.0 * s);
        let cols = ((body.width() + gap) / (cw + gap)).floor().max(1.0) as usize;
        if games.is_empty() {
            clip.text(pos2(body.left(), y), Align2::LEFT_TOP, "Библиотека пуста", font(18.0 * s), white(0.55 * a));
        }
        let row_h = ch + 64.0 * s;
        for (i, g) in games.iter().enumerate() {
            let (c, r) = (i % cols, i / cols);
            let cover = Rect::from_min_size(pos2(body.left() + c as f32 * (cw + gap), y + r as f32 * row_h), vec2(cw, ch));
            if cover.bottom() + 60.0 * s < body.top() || cover.top() > body.bottom() {
                continue;
            }
            let gname = g["n"].as_str().unwrap_or("");
            let sid = g["s"].as_str().map(str::to_string).or_else(|| g["s"].as_u64().map(|n| n.to_string()));
            let mut uris: Vec<String> = sid.as_deref().map(|id| crate::steam::cover_candidates(self.steam_root.as_deref(), id)).unwrap_or_default().into_iter().filter_map(|u| crate::data::image_uri(&u)).collect();
            if let Some(id) = &sid {
                uris.push(format!("https://cdn.cloudflare.steamstatic.com/steam/apps/{id}/header.jpg"));
            }
            let shown = !uris.is_empty() && body.contains_rect(cover) && draw::cover(ui, &uris, cover, 12.0 * s, white(a), (0.5, 0.5));
            if !shown {
                clip.rect_filled(cover, CornerRadius::same((12.0 * s) as u8), draw::name_color(gname).gamma_multiply(a));
                clip.text(cover.center(), Align2::CENTER_CENTER, draw::initials(gname), bold(40.0 * s), white(0.9 * a));
            }
            let nm = clip.layout_no_wrap(gname.to_string(), semibold(15.0 * s), white(a));
            let nclip = clip.with_clip_rect(Rect::from_min_size(pos2(cover.left(), cover.bottom()), vec2(cw, 60.0 * s)).intersect(body));
            nclip.galley(pos2(cover.left(), cover.bottom() + 8.0 * s), nm, Color32::WHITE);
            let sub: Vec<String> = [g["p"].as_f64().filter(|p| *p > 0.0).map(hours), g["l"].as_f64().map(super::fmt_date)].into_iter().flatten().collect();
            nclip.text(pos2(cover.left(), cover.bottom() + 32.0 * s), Align2::LEFT_TOP, sub.join(" · "), font(13.0 * s), white(0.55 * a));
        }
        let rows = games.len().div_ceil(cols).max(1);
        let content = (y + sc + rows as f32 * row_h) - body.top();
        let max = (content - body.height()).max(0.0);
        sc = sc.min(max);
        if let Some(Modal::Friend { scroll, .. }) = self.modal.as_mut() {
            *scroll = sc.min(*scroll);
        }
    }
}

impl App {
    /// `OTIUM_OPEN_PROFILE=<public key>`: opens that player's profile once signed in (for testing).
    pub(super) fn debug_open_profile(&mut self) {
        static DONE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
        let Some(pk) = std::env::var("OTIUM_OPEN_PROFILE").ok() else { return };
        if self.social.me.is_none() || self.splash_start.is_some() || self.modal.is_some() || DONE.swap(true, std::sync::atomic::Ordering::Relaxed) {
            return;
        }
        self.open_friend_profile(&pk);
    }
}

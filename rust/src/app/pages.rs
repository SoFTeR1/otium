//! The pages below the home screen, switched with up/down and shown as dots on the right:
//! 0 = games, 1 = news and trophies of the selected game, 2 = what's playing.

use super::{App, Zone};
use crate::draw::{self, bold, font, semibold, white};
use crate::input::Nav;
use crate::media::{MediaCmd, Music};
use crate::modal::Modal;
use crate::steam_api::{ApiError, News, Trophies};
use crate::watch::Event;
use egui::{pos2, vec2, Align2, Color32, CornerRadius, Rect, Stroke, StrokeKind};
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

pub const PAGES: usize = 3;
const BLUE: Color32 = Color32::from_rgb(61, 123, 255);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Row {
    News,
    Trophies,
}

#[derive(Default)]
pub struct Pages {
    pub page: usize,
    /// animated page position (0.0 ..= 2.0)
    pub pos: f32,
    /// page 1: focused row and card in each row
    row: usize,
    idx: HashMap<u8, usize>,
    xscroll: HashMap<u8, f32>,
    /// page 2: focused control (0 prev, 1 play/pause, 2 next)
    ctl: usize,

    news: HashMap<String, Vec<News>>,
    trophies: HashMap<String, Result<Trophies, ApiError>>,
    loading: HashSet<String>,
    /// Steam app id the news/trophies page is showing, and when it was picked (requests wait a moment
    /// so scrolling through the row doesn't fire one per game)
    current: Option<String>,
    picked_at: Option<Instant>,
    /// trophies to re-read without cache (a game just closed)
    fresh: HashSet<String>,

    pub music: Option<Music>,
}

fn row_key(r: Row) -> u8 {
    r as u8
}

impl App {
    pub(super) fn steam_key(&self) -> String {
        self.lib.setting_str("steamApiKey").trim().to_string()
    }

    pub(super) fn steam_user(&self) -> String {
        let s = self.lib.setting_str("steamId").trim().to_string();
        if s.is_empty() {
            self.steam_user_local.clone().unwrap_or_default()
        } else {
            s
        }
    }

    fn selected_app_id(&self) -> Option<String> {
        self.games.get(self.sel).and_then(|g| g.steam_app_id())
    }

    /// Rows of page 1 that have something to show.
    fn rows(&self) -> Vec<Row> {
        let Some(id) = self.selected_app_id() else { return Vec::new() };
        let mut rows = Vec::new();
        if self.pages.news.get(&id).is_some_and(|n| !n.is_empty()) {
            rows.push(Row::News);
        }
        let trophies = match self.pages.trophies.get(&id) {
            Some(Err(ApiError::None)) => false,
            Some(_) => true,
            // no key: a card explaining how to get one; loading: a placeholder until they arrive
            None => self.steam_key().is_empty() || self.pages.loading.contains(&id),
        };
        if trophies {
            rows.push(Row::Trophies);
        }
        rows
    }

    fn row_len(&self, r: Row) -> usize {
        let Some(id) = self.selected_app_id() else { return 0 };
        match r {
            Row::News => self.pages.news.get(&id).map_or(0, Vec::len),
            Row::Trophies => match self.pages.trophies.get(&id) {
                Some(Ok(t)) => 1 + trophy_cards(t).len(),
                _ => 1,
            },
        }
    }

    pub(super) fn go_page(&mut self, p: usize) {
        self.pages.page = p.min(PAGES - 1);
        self.pages.row = 0;
        if p == 0 {
            self.zone = Zone::Play;
        }
        if p == 2 {
            self.pages.ctl = 1; // play / pause
            let _ = self.media_tx.send(MediaCmd::Poll);
        }
    }

    pub(super) fn handle_page(&mut self, nav: Nav) {
        match self.pages.page {
            1 => {
                let rows = self.rows();
                let row = rows.get(self.pages.row).copied();
                match nav {
                    Nav::Up if self.pages.row > 0 => self.pages.row -= 1,
                    Nav::Up | Nav::Back => self.go_page(0),
                    Nav::Down if self.pages.row + 1 < rows.len() => self.pages.row += 1,
                    Nav::Down => self.go_page(2),
                    Nav::Left | Nav::Right => {
                        let Some(r) = row else { return };
                        let n = self.row_len(r);
                        let i = self.pages.idx.entry(row_key(r)).or_insert(0);
                        if nav == Nav::Left && *i > 0 {
                            *i -= 1;
                        } else if nav == Nav::Right && *i + 1 < n {
                            *i += 1;
                        }
                    }
                    Nav::Ok => {
                        let Some(r) = row else { return };
                        let i = self.pages.idx.get(&row_key(r)).copied().unwrap_or(0);
                        self.open_card(r, i);
                    }
                    _ => {}
                }
            }
            2 => match nav {
                Nav::Up => self.go_page(1),
                Nav::Back => self.go_page(0),
                Nav::Left if self.pages.ctl > 0 => self.pages.ctl -= 1,
                Nav::Right if self.pages.ctl < 2 => self.pages.ctl += 1,
                Nav::Ok if self.pages.music.is_some() => {
                    let cmd = [MediaCmd::Prev, MediaCmd::Toggle, MediaCmd::Next][self.pages.ctl];
                    let _ = self.media_tx.send(cmd);
                }
                _ => {}
            },
            _ => {}
        }
    }

    fn open_card(&mut self, r: Row, i: usize) {
        let Some(id) = self.selected_app_id() else { return };
        let game = self.games.get(self.sel).map(|g| g.name.clone()).unwrap_or_default();
        match r {
            Row::News => {
                if let Some(n) = self.pages.news.get(&id).and_then(|l| l.get(i)).cloned() {
                    let fallback = self.games.get(self.sel).map(|g| self.art(g).backgrounds).unwrap_or_default();
                    self.open(Modal::News { news: n, game, fallback, scroll: 0.0 });
                }
            }
            Row::Trophies => match self.pages.trophies.get(&id) {
                Some(Ok(t)) => {
                    let mut items = t.items.clone();
                    items.sort_by(|a, b| {
                        b.achieved.cmp(&a.achieved).then(b.time.total_cmp(&a.time)).then(b.percent.unwrap_or(0.0).total_cmp(&a.percent.unwrap_or(0.0)))
                    });
                    // a trophy card opens the list at that trophy
                    let focus = if i == 0 {
                        0
                    } else {
                        let name = &trophy_cards(t)[i - 1].0.name;
                        items.iter().position(|a| &a.name == name).unwrap_or(0)
                    };
                    let title = format!("{game} · {}/{}", t.done, t.items.len());
                    self.open(Modal::Trophies { title, items, focus, scroll: 0.0 });
                }
                Some(Err(e)) => {
                    let (title, text) = api_error_text(*e);
                    self.open(Modal::Message { title: title.into(), text: text.into() });
                }
                None => self.open(Modal::Message {
                    title: "Трофеи Steam".into(),
                    text: "Для трофеев нужен бесплатный Steam Web API-ключ (steamcommunity.com/dev/apikey). \
                           Пока его можно указать в старой версии OTIUM: Настройки → Аккаунт Steam — новая версия подхватит его сама."
                        .into(),
                }),
            },
        }
    }

    /// Asks for news and trophies of the selected game once the selection has settled.
    pub(super) fn tick_pages(&mut self) {
        let id = self.selected_app_id();
        if id != self.pages.current {
            self.pages.current = id;
            self.pages.picked_at = Some(Instant::now());
            self.pages.row = 0;
            self.pages.idx.clear();
            self.pages.xscroll.clear();
        }
        let ready = self.pages.picked_at.is_some_and(|t| t.elapsed() > Duration::from_millis(350));
        let Some(id) = self.pages.current.clone().filter(|_| ready) else { return };
        if self.pages.loading.contains(&id) {
            return;
        }
        let need_news = !self.pages.news.contains_key(&id);
        let fresh = self.pages.fresh.remove(&id);
        let (key, user) = (self.steam_key(), self.steam_user());
        let need_trophies = (fresh || !self.pages.trophies.contains_key(&id)) && !key.is_empty() && !user.is_empty();
        if !need_news && !need_trophies {
            return;
        }
        self.pages.loading.insert(id.clone());
        let bus = self.bus.clone();
        std::thread::spawn(move || {
            if need_news {
                let items = crate::steam_api::news(&id);
                bus.send(Event::News { appid: id.clone(), items });
            }
            if need_trophies {
                let res = crate::steam_api::achievements(&id, &key, &user, fresh);
                bus.send(Event::Trophies { appid: id.clone(), res });
            }
            bus.send(Event::FeedDone { appid: id });
        });
    }

    pub(super) fn on_feed_event(&mut self, e: Event) {
        match e {
            Event::News { appid, items } => {
                self.pages.news.insert(appid, items);
            }
            Event::Trophies { appid, res } => {
                // the trophies page also keeps "last trophy" up to date, quietly
                if let Ok(t) = &res {
                    let game = self.lib.games.iter().find(|g| g.steam_app_id().as_deref() == Some(appid.as_str())).map(|g| g.name.clone()).unwrap_or_default();
                    let items = t.items.clone();
                    self.note_trophy(&game, &appid, &items, false);
                }
                self.pages.trophies.insert(appid, res);
            }
            Event::FeedDone { appid } => {
                self.pages.loading.remove(&appid);
            }
            Event::Music(m) => self.pages.music = m,
            _ => {}
        }
        // a row may have disappeared
        let n = self.rows().len();
        self.pages.row = self.pages.row.min(n.saturating_sub(1));
    }

    /// A Steam game closed: its trophies may have changed.
    pub(super) fn trophies_changed(&mut self, appid: u32) {
        self.pages.fresh.insert(appid.to_string());
    }

    // ---------- painting ----------

    pub(super) fn paint_pages(&mut self, ui: &egui::Ui, rect: Rect, s: f32, dt: f32) {
        let k = 1.0 - (-dt * 12.0).exp();
        self.pages.pos += (self.pages.page as f32 - self.pages.pos) * k;
        if (self.pages.pos - self.pages.page as f32).abs() < 0.001 {
            self.pages.pos = self.pages.page as f32;
        }
        let h = rect.height();
        let y1 = h * (1.0 - self.pages.pos);
        if y1 < h && y1 > -h {
            self.paint_news_page(ui, rect.translate(vec2(0.0, y1)), s, dt);
        }
        let y2 = h * (2.0 - self.pages.pos);
        if y2 < h && y2 > -h {
            self.paint_music_page(ui, rect.translate(vec2(0.0, y2)), s);
        }
        self.paint_dots(ui, rect, s);
    }

    fn paint_dots(&self, ui: &egui::Ui, rect: Rect, s: f32) {
        let p = ui.painter();
        let x = rect.right() - 44.0 * s;
        let gap = 30.0 * s;
        let top = rect.center().y - gap * (PAGES as f32 - 1.0) / 2.0;
        for i in 0..PAGES {
            let c = pos2(x, top + i as f32 * gap);
            let near = 1.0 - (self.pages.pos - i as f32).abs().min(1.0);
            let r = (4.0 + 3.0 * near) * s;
            p.circle_filled(c, r + 1.5 * s, Color32::from_black_alpha(90));
            p.circle_filled(c, r, white(0.35 + 0.65 * near));
        }
    }

    fn paint_news_page(&mut self, ui: &egui::Ui, r: Rect, s: f32, dt: f32) {
        let left = r.left() + 96.0 * s;
        let p = ui.painter();
        let game = self.games.get(self.sel).map(|g| g.name.clone()).unwrap_or_default();
        p.text(pos2(left, r.top() + 140.0 * s), Align2::LEFT_TOP, &game, bold(46.0 * s), white(1.0));
        p.text(pos2(left, r.top() + 200.0 * s), Align2::LEFT_TOP, "Новости и достижения", font(22.0 * s), white(0.6));

        let Some(id) = self.selected_app_id() else {
            p.text(r.center(), Align2::CENTER_CENTER, "Новости и трофеи есть у игр из Steam", font(24.0 * s), white(0.6));
            return;
        };
        let rows = self.rows();
        if rows.is_empty() {
            let text = if self.pages.loading.contains(&id) || self.pages.current.as_deref() == Some(&id) && !self.pages.news.contains_key(&id) {
                "Загрузка…"
            } else {
                "Новостей пока нет"
            };
            p.text(r.center(), Align2::CENTER_CENTER, text, font(24.0 * s), white(0.6));
            return;
        }
        let focused_page = self.pages.page == 1 && self.modal.is_none();
        let mut y = r.top() + 290.0 * s;
        for (ri, row) in rows.iter().enumerate() {
            let focused_row = focused_page && ri == self.pages.row;
            let title = match row {
                Row::News => "Новости".to_string(),
                Row::Trophies => match self.pages.trophies.get(&id) {
                    Some(Ok(t)) => format!("Трофеи · {} из {}", t.done, t.items.len()),
                    _ => "Трофеи".to_string(),
                },
            };
            ui.painter().text(pos2(left, y), Align2::LEFT_TOP, title, semibold(26.0 * s), white(if focused_row { 1.0 } else { 0.7 }));
            let cards_y = y + 48.0 * s;
            let card_h = if *row == Row::News { 300.0 } else { 210.0 } * s;
            let focus = self.pages.idx.get(&row_key(*row)).copied().unwrap_or(0);
            let widths: Vec<f32> = match row {
                Row::News => vec![400.0 * s; self.row_len(*row)],
                Row::Trophies => (0..self.row_len(*row)).map(|i| if i == 0 { 460.0 } else { 220.0 } * s).collect(),
            };
            let gap = 24.0 * s;
            // keep the focused card in view: scroll so it sits a little after the left edge
            let x_focus: f32 = widths.iter().take(focus).map(|w| w + gap).sum();
            let target = (x_focus - 0.6 * widths.get(focus).copied().unwrap_or(0.0)).max(0.0);
            let xs = self.pages.xscroll.entry(row_key(*row)).or_insert(0.0);
            *xs += (target - *xs) * (1.0 - (-dt * 14.0).exp());
            let mut x = left - *xs;
            for (i, w) in widths.iter().enumerate() {
                let card = Rect::from_min_size(pos2(x, cards_y), vec2(*w, card_h));
                x += w + gap;
                if card.right() < r.left() || card.left() > r.right() {
                    continue;
                }
                let f = focused_row && i == focus;
                match row {
                    Row::News => {
                        if let Some(n) = self.pages.news.get(&id).and_then(|l| l.get(i)) {
                            let fallback = self.games.get(self.sel).map(|g| self.art(g).backgrounds).unwrap_or_default();
                            paint_news_card(ui, card, s, n, &fallback, f);
                        }
                    }
                    Row::Trophies => self.paint_trophy_card(ui, card, s, &id, i, f),
                }
            }
            y = cards_y + card_h + 56.0 * s;
        }
    }

    fn paint_trophy_card(&self, ui: &egui::Ui, card: Rect, s: f32, id: &str, i: usize, focused: bool) {
        let p = ui.painter();
        card_bg(p, card, s, focused);
        match self.pages.trophies.get(id) {
            Some(Ok(t)) if i == 0 => {
                let total = t.items.len().max(1);
                let pct = (100.0 * t.done as f32 / total as f32).round();
                let c = pos2(card.left() + 100.0 * s, card.center().y);
                let rad = 62.0 * s;
                p.circle_stroke(c, rad, Stroke::new(10.0 * s, white(0.12)));
                arc(p, c, rad, pct / 100.0, Stroke::new(10.0 * s, BLUE));
                p.text(c, Align2::CENTER_CENTER, format!("{pct}%"), semibold(26.0 * s), Color32::WHITE);
                let tx = card.left() + 196.0 * s;
                p.text(pos2(tx, card.top() + 50.0 * s), Align2::LEFT_TOP, format!("{} / {}", t.done, t.items.len()), bold(38.0 * s), Color32::WHITE);
                p.text(pos2(tx, card.top() + 100.0 * s), Align2::LEFT_TOP, "трофеев получено", font(19.0 * s), white(0.65));
                let rest = if t.done == t.items.len() { "🏆 Платина!".to_string() } else { format!("Осталось {}", t.items.len() - t.done) };
                p.text(pos2(tx, card.top() + 130.0 * s), Align2::LEFT_TOP, rest, font(19.0 * s), white(0.65));
            }
            Some(Ok(t)) => {
                let cards = trophy_cards(t);
                let Some((a, tag)) = cards.get(i - 1) else { return };
                let icon = Rect::from_min_size(card.left_top() + vec2(20.0 * s, 20.0 * s), vec2(72.0 * s, 72.0 * s));
                let uris: Vec<String> = a.icon.iter().cloned().collect();
                if !draw::cover(ui, &uris, icon, 10.0 * s, Color32::WHITE, (0.5, 0.5)) {
                    p.rect_filled(icon, CornerRadius::same((10.0 * s) as u8), white(0.1));
                }
                clip_text(ui, card, pos2(card.left() + 20.0 * s, card.top() + 108.0 * s), &a.name, semibold(19.0 * s), Color32::WHITE, 2);
                p.text(pos2(card.left() + 20.0 * s, card.bottom() - 20.0 * s), Align2::LEFT_BOTTOM, tag, font(16.0 * s), white(0.6));
            }
            other => {
                let (title, text) = match other {
                    Some(Err(e)) => api_error_text(*e),
                    _ if self.pages.loading.contains(id) => ("Загрузка трофеев…", "Спрашиваем Steam"),
                    _ => ("Трофеи Steam", "Нужен Steam API-ключ — нажмите, чтобы узнать как"),
                };
                p.text(pos2(card.left() + 24.0 * s, card.top() + 40.0 * s), Align2::LEFT_TOP, "🔑", font(34.0 * s), Color32::WHITE);
                clip_text(ui, card, pos2(card.left() + 24.0 * s, card.top() + 96.0 * s), title, semibold(21.0 * s), Color32::WHITE, 1);
                clip_text(ui, card, pos2(card.left() + 24.0 * s, card.top() + 130.0 * s), text, font(17.0 * s), white(0.65), 2);
            }
        }
    }

    fn paint_music_page(&self, ui: &egui::Ui, r: Rect, s: f32) {
        let p = ui.painter();
        let Some(m) = &self.pages.music else {
            p.text(r.center() - vec2(0.0, 20.0 * s), Align2::CENTER_CENTER, "Сейчас ничего не играет", semibold(32.0 * s), white(0.85));
            p.text(
                r.center() + vec2(0.0, 26.0 * s),
                Align2::CENTER_CENTER,
                "Включите музыку в Spotify, Яндекс Музыке или браузере — она появится здесь",
                font(20.0 * s),
                white(0.55),
            );
            return;
        };
        let size = 360.0 * s;
        let art = Rect::from_center_size(pos2(r.center().x, r.top() + r.height() * 0.40), vec2(size, size));
        draw::glow(p, art, 24.0 * s, BLUE, 60.0 * s, if m.playing { 0.8 } else { 0.3 });
        let uris: Vec<String> = m.art.iter().cloned().collect();
        if !draw::cover(ui, &uris, art, 24.0 * s, Color32::WHITE, (0.5, 0.5)) {
            p.rect_filled(art, CornerRadius::same((24.0 * s) as u8), Color32::from_rgb(28, 34, 52));
            p.text(art.center(), Align2::CENTER_CENTER, "♪", bold(120.0 * s), white(0.5));
        }
        let ty = art.bottom() + 40.0 * s;
        p.text(pos2(r.center().x, ty), Align2::CENTER_TOP, &m.title, bold(38.0 * s), Color32::WHITE);
        let sub = [m.artist.as_str(), m.album.as_str()].iter().filter(|x| !x.is_empty()).copied().collect::<Vec<_>>().join(" — ");
        p.text(pos2(r.center().x, ty + 52.0 * s), Align2::CENTER_TOP, sub, font(22.0 * s), white(0.7));
        let state = format!("{} · {}", crate::media::app_name(&m.app), if m.playing { "играет" } else { "пауза" });
        p.text(pos2(r.center().x, ty + 88.0 * s), Align2::CENTER_TOP, state, font(18.0 * s), white(0.5));

        // prev / play-pause / next
        let cy = ty + 170.0 * s;
        let focused = self.pages.page == 2 && self.modal.is_none();
        for (i, dx) in [-120.0, 0.0, 120.0].into_iter().enumerate() {
            let c = pos2(r.center().x + dx * s, cy);
            let rad = if i == 1 { 46.0 } else { 36.0 } * s;
            let f = focused && self.pages.ctl == i;
            if f {
                draw::glow(p, Rect::from_center_size(c, vec2(rad * 2.0, rad * 2.0)), rad, Color32::WHITE, 16.0 * s, 0.6);
            }
            p.circle_filled(c, rad, if f { Color32::WHITE } else { white(0.12) });
            let fg = if f { Color32::from_rgb(10, 12, 18) } else { Color32::WHITE };
            media_icon(p, c, rad * 0.42, i, m.playing, fg);
        }
    }
}

/// Recent trophies, then the rarest ones not already shown.
fn trophy_cards(t: &Trophies) -> Vec<(crate::steam_api::Achievement, String)> {
    let got: Vec<_> = t.items.iter().filter(|a| a.achieved).cloned().collect();
    let mut recent = got.clone();
    recent.sort_by(|a, b| b.time.total_cmp(&a.time));
    recent.truncate(5);
    let mut rare: Vec<_> = got.into_iter().filter(|a| a.percent.is_some() && !recent.iter().any(|r| r.name == a.name)).collect();
    rare.sort_by(|a, b| a.percent.unwrap_or(100.0).total_cmp(&b.percent.unwrap_or(100.0)));
    rare.truncate(3);
    let mut out: Vec<_> = recent.into_iter().map(|a| { let tag = fmt_date(a.time); (a, tag) }).collect();
    out.extend(rare.into_iter().map(|a| { let tag = format!("Редкий · {:.1}%", a.percent.unwrap_or(0.0)); (a, tag) }));
    out
}

fn api_error_text(e: ApiError) -> (&'static str, &'static str) {
    match e {
        ApiError::BadKey => ("Steam не принял API-ключ", "Проверьте ключ в настройках аккаунта Steam"),
        ApiError::Private => ("Сведения об играх скрыты", "Откройте «Сведения об играх» в настройках приватности Steam"),
        ApiError::Network => ("Нет связи со Steam", "Попробуем ещё раз позже"),
        ApiError::Other | ApiError::None => ("Steam вернул ошибку", "Проверьте аккаунт Steam"),
    }
}

fn card_bg(p: &egui::Painter, card: Rect, s: f32, focused: bool) {
    let cr = CornerRadius::same((16.0 * s) as u8);
    if focused {
        draw::glow(p, card, 16.0 * s, Color32::WHITE, 16.0 * s, 0.5);
    }
    p.rect_filled(card, cr, Color32::from_rgba_unmultiplied(28, 32, 46, 230));
    if focused {
        p.rect_stroke(card, cr, Stroke::new(3.0 * s, Color32::WHITE), StrokeKind::Outside);
    }
}

fn paint_news_card(ui: &egui::Ui, card: Rect, s: f32, n: &News, fallback: &[String], focused: bool) {
    let p = ui.painter();
    card_bg(p, card, s, focused);
    let img = Rect::from_min_size(card.min, vec2(card.width(), card.height() * 0.58));
    // the article image when it loads, the game art until then
    let mut uris: Vec<String> = n.image.iter().cloned().collect();
    uris.extend(fallback.iter().cloned());
    if !draw::cover(ui, &uris, img, 16.0 * s, Color32::WHITE, (0.5, 0.3)) {
        p.rect_filled(img, CornerRadius::same((16.0 * s) as u8), white(0.06));
    }
    clip_text(ui, card, pos2(card.left() + 18.0 * s, img.bottom() + 14.0 * s), &n.title, semibold(19.0 * s), Color32::WHITE, 2);
    let meta = [n.feed.clone(), fmt_date(n.date)].into_iter().filter(|x| !x.is_empty()).collect::<Vec<_>>().join(" · ");
    p.text(pos2(card.left() + 18.0 * s, card.bottom() - 16.0 * s), Align2::LEFT_BOTTOM, meta, font(15.0 * s), white(0.55));
}

/// Text wrapped to the card width and cut to `lines` lines with an ellipsis.
fn clip_text(ui: &egui::Ui, card: Rect, at: egui::Pos2, text: &str, f: egui::FontId, color: Color32, lines: usize) {
    let mut job = egui::text::LayoutJob::simple(text.to_string(), f, color, card.right() - at.x - 18.0);
    job.wrap.max_rows = lines;
    job.wrap.break_anywhere = false;
    job.wrap.overflow_character = Some('…');
    let galley = ui.fonts(|fo| fo.layout_job(job));
    ui.painter().galley(at, galley, color);
}

fn arc(p: &egui::Painter, c: egui::Pos2, r: f32, frac: f32, stroke: Stroke) {
    if frac <= 0.0 {
        return;
    }
    let n = (64.0 * frac).ceil().max(2.0) as usize;
    let pts: Vec<egui::Pos2> = (0..=n)
        .map(|i| {
            let a = -std::f32::consts::FRAC_PI_2 + std::f32::consts::TAU * frac * i as f32 / n as f32;
            c + vec2(a.cos(), a.sin()) * r
        })
        .collect();
    p.add(egui::Shape::line(pts, stroke));
}

/// 0 = previous, 1 = play/pause, 2 = next
fn media_icon(p: &egui::Painter, c: egui::Pos2, r: f32, which: usize, playing: bool, color: Color32) {
    let tri = |dir: f32, cx: f32| {
        vec![pos2(cx - dir * r * 0.5, c.y - r * 0.7), pos2(cx - dir * r * 0.5, c.y + r * 0.7), pos2(cx + dir * r * 0.6, c.y)]
    };
    let poly = |pts: Vec<egui::Pos2>| p.add(egui::Shape::convex_polygon(pts, color, Stroke::NONE));
    match which {
        1 if playing => {
            for dx in [-0.45, 0.45] {
                p.rect_filled(Rect::from_center_size(c + vec2(dx * r, 0.0), vec2(r * 0.4, r * 1.5)), 2.0, color);
            }
        }
        1 => {
            poly(tri(1.0, c.x + r * 0.1));
        }
        0 => {
            p.rect_filled(Rect::from_center_size(c + vec2(-r * 0.75, 0.0), vec2(r * 0.25, r * 1.4)), 1.0, color);
            poly(tri(-1.0, c.x + r * 0.15));
        }
        _ => {
            p.rect_filled(Rect::from_center_size(c + vec2(r * 0.75, 0.0), vec2(r * 0.25, r * 1.4)), 1.0, color);
            poly(tri(1.0, c.x - r * 0.15));
        }
    }
}

/// "сегодня, 14:05", "вчера", "3 дн. назад", "4 ноября 2026"
pub fn fmt_date(ms: f64) -> String {
    use chrono::{Datelike, Local, TimeZone};
    if ms <= 0.0 {
        return String::new();
    }
    let Some(d) = Local.timestamp_millis_opt(ms as i64).single() else { return String::new() };
    let days = ((Local::now().timestamp_millis() as f64 - ms) / 86_400_000.0).floor() as i64;
    const MONTHS: [&str; 12] = [
        "января", "февраля", "марта", "апреля", "мая", "июня", "июля", "августа", "сентября", "октября", "ноября", "декабря",
    ];
    match days {
        0 => format!("сегодня, {}", d.format("%H:%M")),
        1 => "вчера".into(),
        2..=6 => format!("{days} дн. назад"),
        _ => format!("{} {} {}", d.day(), MONTHS[d.month0() as usize], d.year()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates() {
        let now = chrono::Local::now().timestamp_millis() as f64;
        assert!(fmt_date(now - 60_000.0).starts_with("сегодня"));
        assert_eq!(fmt_date(now - 86_400_000.0 * 1.5), "вчера");
        assert_eq!(fmt_date(now - 86_400_000.0 * 3.5), "3 дн. назад");
        assert!(fmt_date(1_700_000_000_000.0).contains("ноября 2023"));
        assert_eq!(fmt_date(0.0), "");
    }
}

impl App {
    /// The Steam key or account changed: trophies are fetched again.
    pub(super) fn pages_forget_trophies(&mut self) {
        self.pages.trophies.clear();
        crate::steam_api::forget("ach");
    }
}

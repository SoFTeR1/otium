//! Friends and chat: the account, friend requests, presence and direct messages over Nostr,
//! and the full-screen "Друзья и сообщения" panel.

use super::App;
use crate::account::{Account, Friend, Message, Request};
use crate::draw::{self, bold, font, semibold, white};
use crate::input::Nav;
use crate::keyboard::Target;
use crate::modal::{Modal, SetItem, SetKey, SetMenu};
use crate::nostr::{self, Cmd, Msg};
use egui::{pos2, vec2, Align2, Color32, CornerRadius, Rect, Stroke};
use serde_json::json;
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant};

const CTRL: &str = "ps5l";
const ONLINE_MS: f64 = 6.0 * 60_000.0;
const BLUE: Color32 = Color32::from_rgb(61, 123, 255);
const GREEN: Color32 = Color32::from_rgb(67, 181, 129);

#[derive(Clone, Copy, PartialEq, Eq)]
enum Zone {
    List,
    Chat,
}

#[derive(Clone, PartialEq)]
enum Item {
    Me,
    Find,
    NewGroup,
    Request(String),
    Group(String),
    Friend(String),
}

pub struct Social {
    pub open: bool,
    zone: Zone,
    idx: usize,
    chat: Option<String>,
    /// lines scrolled up from the bottom of the chat
    chat_scroll: f32,
    presence: HashMap<String, (String, f64)>,
    profiles: HashMap<String, (String, u64)>,
    seen: HashSet<String>,
    me: Option<String>,
    net: nostr::Nostr,
    presence_at: Option<Instant>,
    save_at: Option<Instant>,
    /// what "find friends" is looking for, while the search runs
    pending_find: Option<String>,
    /// friend the friend menu was opened for
    pub menu_pk: Option<String>,
    login_pending: Option<String>,
    /// the parsed account; library.json holds the source of truth, this saves re-parsing every frame
    cache: std::cell::RefCell<Option<Option<Account>>>,
}

impl Social {
    pub fn me_pk(&self) -> Option<String> {
        self.me.clone()
    }

    /// Sends a raw (already JSON) control message as an encrypted DM.
    pub fn send_raw(&self, pk: &str, text: String) {
        self.net.send(Cmd::SendDm { to: pk.into(), text });
    }

    pub fn chat_is(&self, key: &str) -> bool {
        self.chat.as_deref() == Some(key)
    }

    pub fn close_chat_if(&mut self, key: &str) {
        if self.chat_is(key) {
            self.chat = None;
            self.zone = Zone::List;
        }
    }

    pub fn open_chat(&mut self, key: &str) {
        self.open = true;
        self.chat = Some(key.into());
        self.zone = Zone::Chat;
        self.chat_scroll = 0.0;
    }

    pub fn chat_focused(&self) -> bool {
        self.zone == Zone::Chat
    }

    pub fn chat_scroll(&self) -> f32 {
        self.chat_scroll
    }

    pub fn set_chat_scroll(&mut self, v: f32) {
        self.chat_scroll = v;
    }

    pub fn new(bus: crate::watch::Bus) -> Self {
        let net = nostr::Nostr::start(move |m| bus.send(crate::watch::Event::Nostr(m)));
        Self {
            open: false,
            zone: Zone::List,
            idx: 0,
            chat: None,
            chat_scroll: 0.0,
            presence: HashMap::new(),
            profiles: HashMap::new(),
            seen: HashSet::new(),
            me: None,
            net,
            presence_at: None,
            save_at: None,
            pending_find: None,
            menu_pk: None,
            login_pending: None,
            cache: Default::default(),
        }
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

fn now_s() -> u64 {
    (now_ms() / 1000) as u64
}

impl App {
    pub(super) fn account(&self) -> Option<Account> {
        let mut c = self.social.cache.borrow_mut();
        if c.is_none() {
            *c = Some(Account::load(self.lib.extra.get("social")));
        }
        c.as_ref().and_then(|a| a.clone())
    }

    pub(super) fn store_account(&mut self, a: Option<&Account>) {
        *self.social.cache.borrow_mut() = Some(a.cloned());
        match a {
            Some(a) => {
                self.lib.extra.insert("social".into(), serde_json::to_value(a).unwrap_or_default());
            }
            None => {
                self.lib.extra.insert("social".into(), serde_json::Value::Null);
            }
        }
        // chats change often: write a moment later, once
        self.social.save_at = Some(Instant::now() + Duration::from_millis(800));
    }

    pub(super) fn with_account(&mut self, f: impl FnOnce(&mut Account)) {
        if let Some(mut a) = self.account() {
            f(&mut a);
            self.store_account(Some(&a));
        }
    }

    /// Connects with the stored account, if there is one.
    pub(super) fn social_start(&mut self) {
        let Some(a) = self.account() else { return };
        let Some(pk) = nostr::public_key(&a.sk) else { return };
        self.social.me = Some(pk);
        self.social.seen = a.history.values().flatten().map(|m| m.id.clone()).collect();
        self.social.net.send(Cmd::Start(a.sk.clone()));
        self.watch_friends(&a);
        self.publish_presence();
        self.publish_library();
        self.social.presence_at = Some(Instant::now());
    }

    pub(super) fn watch_friends(&self, a: &Account) {
        let pks: Vec<String> = a.friends.iter().map(|f| f.pk.clone()).chain(a.requests.iter().map(|r| r.pk.clone())).collect();
        self.social.net.send(Cmd::Watch(pks));
    }

    pub(super) fn social_tick(&mut self) {
        if self.social.save_at.is_some_and(|t| Instant::now() >= t) {
            self.social.save_at = None;
            self.save();
        }
        // presence lives 10 minutes on the relays; refresh it every 2
        if self.social.me.is_some() && self.social.presence_at.is_some_and(|t| t.elapsed() > Duration::from_secs(120)) {
            self.publish_presence();
            self.social.presence_at = Some(Instant::now());
        }
    }

    fn my_status(&self) -> String {
        match self.games.iter().find(|g| self.running.contains(&g.id)) {
            Some(g) => format!("В игре: {}", g.name),
            None => "В сети".into(),
        }
    }

    pub(super) fn publish_presence(&self) {
        if self.social.me.is_none() {
            return;
        }
        let exp = (now_s() + 600).to_string();
        self.social.net.send(Cmd::Publish(30315, self.my_status(), vec![vec!["d".into(), "general".into()], vec!["expiration".into(), exp]]));
    }

    fn publish_profile(&self, name: &str) {
        let content = json!({ "name": name, "display_name": name, "about": "OTIUM" }).to_string();
        self.social.net.send(Cmd::Publish(0, content, vec![vec!["t".into(), nostr::APP_TAG.into()]]));
    }

    fn publish_contacts(&self, a: &Account) {
        let tags = a.friends.iter().map(|f| vec!["p".into(), f.pk.clone(), String::new(), f.name.clone()]).collect();
        self.social.net.send(Cmd::Publish(3, String::new(), tags));
    }

    /// Friends can open our profile in the Electron build: the game list and playtime.
    pub(super) fn publish_library(&self) {
        if self.social.me.is_none() {
            return;
        }
        let mut games: Vec<&crate::data::Game> = self.lib.games.iter().filter(|g| g.category() != "app").collect();
        games.sort_by(|a, b| b.last_played.unwrap_or(0.0).total_cmp(&a.last_played.unwrap_or(0.0)));
        let list: Vec<serde_json::Value> = games
            .iter()
            .take(40)
            .map(|g| {
                let p = self.playtime(g).round() as i64;
                json!({ "n": g.name, "s": g.steam_app_id(), "p": (p > 0).then_some(p), "l": g.last_played.map(|x| x as i64) })
            })
            .collect();
        let content = json!({ "games": list, "since": self.lib.settings.get("since"), "trophy": self.lib.extra.get("lastTrophy") }).to_string();
        self.social.net.send(Cmd::Publish(30078, content, vec![vec!["d".into(), "ps5l-library".into()]]));
    }

    pub(super) fn send_ctrl(&self, pk: &str, t: &str) {
        let name = self.account().map(|a| a.name).unwrap_or_default();
        let text = json!({ "pl": CTRL, "t": t, "n": name }).to_string();
        self.social.net.send(Cmd::SendDm { to: pk.into(), text });
    }

    pub(super) fn presence_of(&self, pk: &str) -> (bool, String, bool) {
        match self.social.presence.get(pk) {
            Some((text, at)) if (now_ms() as f64 - at) < ONLINE_MS => {
                let t = if text.is_empty() { "В сети".to_string() } else { text.clone() };
                let game = t.starts_with("В игре");
                (true, t, game)
            }
            _ => (false, "Не в сети".into(), false),
        }
    }

    pub(super) fn display_name(&self, a: &Account, pk: &str) -> String {
        a.name_of(pk)
            .map(str::to_string)
            .or_else(|| self.social.profiles.get(pk).map(|(n, _)| n.clone()))
            .unwrap_or_else(|| format!("Игрок {}", &pk[..6.min(pk.len())]))
    }

    pub fn social_unread(&self) -> usize {
        let (Some(a), Some(me)) = (self.account(), self.social.me.as_deref()) else { return 0 };
        a.friends.iter().map(|f| a.unread(&f.pk, me)).sum::<usize>() + a.requests.len() + a.groups().iter().map(|g| a.group_unread(g, me)).sum::<usize>()
    }

    // ---------- network events ----------

    pub(super) fn on_nostr(&mut self, m: Msg) {
        match m {
            Msg::Dm { id, other, mine, text, at } => self.on_dm(id, other, mine, text, at as i64),
            Msg::Profile { pk, name, at } => {
                if self.social.profiles.get(&pk).is_none_or(|(_, t)| *t < at) {
                    self.social.profiles.insert(pk.clone(), (name.clone(), at));
                }
                let rename = self.account().is_some_and(|a| a.friends.iter().any(|f| f.pk == pk && f.name != name));
                if rename {
                    self.with_account(|a| {
                        if let Some(f) = a.friends.iter_mut().find(|f| f.pk == pk) {
                            f.name = name;
                        }
                    });
                }
            }
            Msg::Presence { pk, text, at } => {
                if self.social.presence.get(&pk).is_none_or(|(_, t)| *t < at) {
                    self.social.presence.insert(pk, (text, at));
                }
            }
            Msg::Players(list) => self.show_players(list),
            Msg::Account { name, friends } => {
                let Some(sk) = self.social.login_pending.take() else { return };
                let a = Account {
                    sk,
                    name: name.unwrap_or_else(|| "Игрок".into()),
                    friends: friends.into_iter().map(|(pk, name)| Friend { pk, name, extra: Default::default() }).collect(),
                    ..Default::default()
                };
                let hello = format!("С возвращением, {}", a.name);
                self.store_account(Some(&a));
                self.social_start();
                self.toast(&hello, false);
            }
            Msg::SendFailed => self.toast("Сообщение не отправлено — нет связи", true),
        }
    }

    fn on_dm(&mut self, id: String, other: String, mine: bool, text: String, at: i64) {
        if !self.social.seen.insert(id.clone()) {
            return;
        }
        let Some(a) = self.account() else { return };
        let recent = |mins: i64| now_ms() - at < mins * 60_000;

        if text.starts_with(&format!("{{\"pl\":\"{CTRL}")) {
            let Ok(c) = serde_json::from_str::<serde_json::Value>(&text) else { return };
            let t = c["t"].as_str().unwrap_or("");
            let n = c["n"].as_str().unwrap_or("").to_string();
            match (t, mine) {
                ("fr", false) => {
                    if a.is_friend(&other) {
                        return;
                    }
                    // both sides asked — that's a yes
                    if a.outgoing.contains(&other) {
                        return self.add_friend(&other, &n, true);
                    }
                    if !a.requests.iter().any(|r| r.pk == other) {
                        let name = if n.is_empty() { self.display_name(&a, &other) } else { n.clone() };
                        self.with_account(|a| a.requests.push(Request { pk: other.clone(), name, at }));
                        if recent(10) {
                            self.toast(&format!("{} хочет добавить вас в друзья", if n.is_empty() { "Игрок" } else { &n }), false);
                        }
                        if let Some(a) = self.account() {
                            self.watch_friends(&a);
                        }
                    }
                }
                ("fr", true) => {
                    if !a.outgoing.contains(&other) && !a.is_friend(&other) {
                        self.with_account(|a| a.outgoing.push(other.clone()));
                    }
                }
                ("fa", false) if a.outgoing.contains(&other) => {
                    self.add_friend(&other, &n, false);
                    if recent(10) {
                        self.toast(&format!("{} теперь у вас в друзьях", if n.is_empty() { "Игрок" } else { &n }), false);
                    }
                }
                ("fa", true) if !a.is_friend(&other) => {
                    let name = a.requests.iter().find(|r| r.pk == other).map(|r| r.name.clone()).unwrap_or_default();
                    self.add_friend(&other, &name, false);
                }
                ("rm", false) if a.is_friend(&other) => self.remove_friend(&other, false),
                (t, false) if super::calls::CALL_CTRL.contains(&t) => self.on_call_ctrl(&other, &c, at),
                (t, false) if super::calls::GROUP_CTRL.contains(&t) => self.on_group_ctrl(&other, &c, at),
                _ => {}
            }
            return;
        }

        if !a.is_friend(&other) {
            return;
        }
        let me = self.social.me.clone().unwrap_or_default();
        let from = if mine { me } else { other.clone() };
        let msg = Message { id, from, text: text.clone(), at };
        let mut added = false;
        self.with_account(|a| added = a.push_message(&other, msg));
        if !added {
            return;
        }
        let viewing = self.social.open && self.social.chat.as_deref() == Some(&other);
        if viewing {
            self.mark_read(&other);
            self.social.chat_scroll = 0.0;
        } else if !mine && recent(5) {
            let name = self.display_name(&a, &other);
            let short: String = text.chars().take(120).collect();
            self.notify(&name, &short, (other.clone(), name.clone()));
            if self.sounds_on() {
                crate::sound::play(crate::sound::Sfx::Ok);
            }
        }
    }

    fn add_friend(&mut self, pk: &str, name: &str, send_accept: bool) {
        let fallback = self.account().map(|a| self.display_name(&a, pk)).unwrap_or_default();
        self.with_account(|a| {
            if !a.is_friend(pk) {
                a.friends.push(Friend { pk: pk.into(), name: if name.is_empty() { fallback } else { name.into() }, extra: Default::default() });
            }
            a.requests.retain(|r| r.pk != pk);
            a.outgoing.retain(|x| x != pk);
        });
        if send_accept {
            self.send_ctrl(pk, "fa");
        }
        if let Some(a) = self.account() {
            self.publish_contacts(&a);
            self.watch_friends(&a);
        }
    }

    fn remove_friend(&mut self, pk: &str, notify: bool) {
        self.with_account(|a| {
            a.friends.retain(|f| f.pk != pk);
            a.history.remove(pk);
        });
        if notify {
            self.send_ctrl(pk, "rm");
        }
        if self.social.chat.as_deref() == Some(pk) {
            self.social.chat = None;
            self.social.zone = Zone::List;
        }
        if let Some(a) = self.account() {
            self.publish_contacts(&a);
            self.watch_friends(&a);
        }
    }

    pub(super) fn mark_read(&mut self, pk: &str) {
        let pk = pk.to_string();
        self.with_account(|a| {
            a.last_read.insert(pk, now_ms());
        });
    }

    // ---------- panel ----------

    pub(super) fn open_social(&mut self) {
        self.modal = None;
        self.social.open = true;
        self.social.zone = Zone::List;
        self.social.idx = 0;
    }

    fn items(&self, a: &Account) -> Vec<Item> {
        let mut items = vec![Item::Me, Item::Find, Item::NewGroup];
        items.extend(a.requests.iter().map(|r| Item::Request(r.pk.clone())));
        let me = self.social.me.clone().unwrap_or_default();
        // groups with people in voice or unread messages first
        let mut gs = a.groups();
        let last_at = |g: &crate::account::Group| g.history.last().map_or(g.created, |m| m.at.max(g.created));
        gs.sort_by(|x, y| {
            self.in_room(&y.id).len().cmp(&self.in_room(&x.id).len()).then(a.group_unread(y, &me).cmp(&a.group_unread(x, &me))).then(last_at(y).cmp(&last_at(x)))
        });
        items.extend(gs.into_iter().map(|g| Item::Group(g.id)));
        let mut fr: Vec<&Friend> = a.friends.iter().collect();
        fr.sort_by(|x, y| {
            let (px, py) = (self.presence_of(&x.pk).0, self.presence_of(&y.pk).0);
            py.cmp(&px).then(a.unread(&y.pk, &me).cmp(&a.unread(&x.pk, &me))).then(x.name.to_lowercase().cmp(&y.name.to_lowercase()))
        });
        items.extend(fr.into_iter().map(|f| Item::Friend(f.pk.clone())));
        items
    }

    /// Navigation while the panel is open (modals on top of it are handled before this).
    pub(super) fn social_nav(&mut self, nav: Nav) {
        let Some(a) = self.account() else {
            // no account yet: 0 = create, 1 = sign in with a key
            match nav {
                Nav::Back => self.social.open = false,
                Nav::Up => self.social.idx = 0,
                Nav::Down => self.social.idx = 1,
                Nav::Ok if self.social.idx == 0 => {
                    let user = self.lib.setting_str("user").to_string();
                    self.open(Modal::Keyboard(Box::new(crate::keyboard::Keyboard::new("Ник для аккаунта OTIUM", &user, Target::AccountName, 24, false))));
                }
                Nav::Ok => self.open(Modal::Keyboard(Box::new(crate::keyboard::Keyboard::new("Ключ входа (nsec…)", "", Target::LoginKey, 80, true)))),
                _ => {}
            }
            return;
        };
        let items = self.items(&a);
        self.social.idx = self.social.idx.min(items.len().saturating_sub(1));
        match self.social.zone {
            Zone::List => match nav {
                Nav::Back => self.social.open = false,
                Nav::Up if self.social.idx > 0 => self.social.idx -= 1,
                Nav::Down if self.social.idx + 1 < items.len() => self.social.idx += 1,
                Nav::Right if self.social.chat.is_some() => self.social.zone = Zone::Chat,
                Nav::Ok => self.activate(&a, items[self.social.idx].clone()),
                Nav::Options => match &items[self.social.idx] {
                    Item::Friend(pk) => self.open_friend_menu(&a, pk),
                    Item::Group(gid) => self.open_group_menu(gid),
                    _ => {}
                },
                _ => {}
            },
            Zone::Chat => match nav {
                Nav::Back | Nav::Left => self.social.zone = Zone::List,
                Nav::Up => self.social.chat_scroll += 3.0,
                Nav::Down => self.social.chat_scroll = (self.social.chat_scroll - 3.0).max(0.0),
                Nav::Ok => {
                    let chat = self.social.chat.clone().unwrap_or_default();
                    let title = match chat.strip_prefix("g:") {
                        Some(gid) => format!("Сообщение в «{}»", self.group(gid).map(|g| g.title(&self.social.me.clone().unwrap_or_default())).unwrap_or_default()),
                        None => format!("Сообщение для {}", self.display_name(&a, &chat)),
                    };
                    self.open(Modal::Keyboard(Box::new(crate::keyboard::Keyboard::new(&title, "", Target::Chat, 1000, false))));
                }
                Nav::Options => {
                    if let Some(chat) = self.social.chat.clone() {
                        match chat.strip_prefix("g:") {
                            // a group chat: Y joins the voice room, or opens its menu once in it
                            Some(gid) if self.calls.room.as_ref().is_some_and(|r| r.gid == gid) => self.open_group_menu(gid),
                            Some(gid) => self.join_room(gid),
                            // a friend: Y opens the call menu during a call with them, else calls
                            None if self.calls.in_call() && self.calls.call.pk == chat => self.open_call_menu(0),
                            None => self.open_friend_menu(&a, &chat),
                        }
                    }
                }
                _ => {}
            },
        }
    }

    fn activate(&mut self, a: &Account, it: Item) {
        match it {
            Item::Me => self.open_profile(a),
            Item::Find => self.open(Modal::Keyboard(Box::new(crate::keyboard::Keyboard::new("Ник или код друга", "", Target::FindFriends, 64, false)))),
            Item::NewGroup => self.open_picker("Новая группа", super::calls::PickFor::NewGroup, &[], super::calls::MAX_GROUP - 1),
            Item::Group(gid) => {
                let key = format!("g:{gid}");
                self.social.open_chat(&key);
                self.mark_read(&key);
            }
            Item::Request(pk) => {
                let name = self.display_name(a, &pk);
                self.open(Modal::FriendRequest { pk, name, focus: 1 });
            }
            Item::Friend(pk) => {
                self.social.chat = Some(pk.clone());
                self.social.zone = Zone::Chat;
                self.social.chat_scroll = 0.0;
                self.mark_read(&pk);
            }
        }
    }

    fn open_profile(&mut self, a: &Account) {
        let me = self.social.me.clone().unwrap_or_default();
        let items = vec![
            SetItem { key: SetKey::ProfileCode, label: "Код друга".into(), value: nostr::short_code(&me), danger: false },
            SetItem { key: SetKey::ProfileRename, label: "Сменить ник".into(), value: a.name.clone(), danger: false },
            SetItem { key: SetKey::ProfileKey, label: "Ключ для входа на другом ПК".into(), value: "показать".into(), danger: false },
            SetItem { key: SetKey::ProfileLogout, label: "Выйти из аккаунта".into(), value: String::new(), danger: true },
        ];
        self.open(Modal::Settings { menu: SetMenu::Profile, title: "Мой профиль".into(), items, focus: 0, scroll: 0.0 });
    }

    fn open_friend_menu(&mut self, a: &Account, pk: &str) {
        self.social.menu_pk = Some(pk.to_string());
        let items = vec![
            SetItem { key: SetKey::FriendChat, label: "Написать".into(), value: String::new(), danger: false },
            SetItem { key: SetKey::FriendCall, label: "Позвонить".into(), value: "голосовой звонок".into(), danger: false },
            SetItem { key: SetKey::FriendRemove, label: "Удалить из друзей".into(), value: String::new(), danger: true },
        ];
        self.open(Modal::Settings { menu: SetMenu::Friend, title: self.display_name(a, pk), items, focus: 0, scroll: 0.0 });
    }

    /// Settings-style menus of the panel (profile, friend).
    pub(super) fn social_setting(&mut self, key: SetKey) {
        let Some(a) = self.account() else { return };
        match key {
            SetKey::ProfileCode => {
                let me = self.social.me.clone().unwrap_or_default();
                self.open(Modal::Message {
                    title: format!("Код друга: {}", nostr::short_code(&me)),
                    text: format!("Друг найдёт вас по нику «{}» или по этому коду в «Найти друзей».\n\nПолный ID: {}", a.name, nostr::npub(&me)),
                });
            }
            SetKey::ProfileRename => {
                self.open(Modal::Keyboard(Box::new(crate::keyboard::Keyboard::new("Новый ник", &a.name, Target::Rename, 24, false))));
            }
            SetKey::ProfileKey => self.open(Modal::Message {
                title: "Ключ входа".into(),
                text: format!("Это пароль от аккаунта. Никому его не показывайте — с ним можно писать от вашего имени.\n\n{}", nostr::nsec(&a.sk)),
            }),
            SetKey::ProfileLogout => self.open(Modal::Confirm {
                title: "Выйти из аккаунта?".into(),
                text: "Без ключа входа вернуться в этот аккаунт будет нельзя. Сохраните ключ, если он понадобится.".into(),
                yes: SetKey::ProfileLogout,
                yes_label: "Выйти",
                focus: 0,
            }),
            SetKey::FriendChat => {
                self.modal = None;
                if let Some(pk) = self.social.menu_pk.take() {
                    self.activate(&a, Item::Friend(pk));
                }
            }
            SetKey::FriendCall => {
                self.modal = None;
                if let Some(pk) = self.social.menu_pk.take() {
                    self.start_call(&pk);
                }
            }
            SetKey::FriendRemove => {
                self.modal = None;
                if let Some(pk) = self.social.menu_pk.take() {
                    self.remove_friend(&pk, true);
                    self.toast("Удалено из друзей", false);
                }
            }
            _ => {}
        }
    }

    pub(super) fn social_logout(&mut self) {
        self.social.net.send(Cmd::Stop);
        self.social.me = None;
        self.social.chat = None;
        self.social.seen.clear();
        self.store_account(None);
        self.modal = None;
        self.social.idx = 0;
    }

    /// Text typed on the on-screen keyboard for the panel.
    pub(super) fn social_text(&mut self, target: Target, text: String) {
        self.modal = None;
        match target {
            Target::Chat => {
                let Some(pk) = self.social.chat.clone() else { return };
                if text.is_empty() {
                    return;
                }
                if let Some(gid) = pk.strip_prefix("g:") {
                    let gid = gid.to_string();
                    self.send_group_message(&gid, &text);
                    self.social.chat_scroll = 0.0;
                    return;
                }
                self.social.net.send(Cmd::SendDm { to: pk.clone(), text });
                self.mark_read(&pk);
                self.social.chat_scroll = 0.0;
            }
            Target::AccountName => {
                if text.chars().count() < 2 {
                    return self.toast("Ник должен быть не короче 2 символов", true);
                }
                let a = Account { sk: nostr::generate_secret(), name: text.clone(), ..Default::default() };
                self.store_account(Some(&a));
                self.social_start();
                self.publish_profile(&text);
                self.publish_contacts(&a);
                self.toast(&format!("Аккаунт «{text}» создан"), false);
            }
            Target::LoginKey => {
                let Some(sk) = nostr::secret_hex(&text) else {
                    return self.toast("Это не похоже на ключ аккаунта", true);
                };
                let Some(pk) = nostr::public_key(&sk) else { return };
                self.toast("Входим…", false);
                self.social.login_pending = Some(sk.clone());
                // connect first so the profile and contact list can be fetched
                self.social.net.send(Cmd::Start(sk));
                self.social.net.send(Cmd::FetchAccount(pk));
            }
            Target::Rename => {
                if text.chars().count() < 2 {
                    return self.toast("Ник должен быть не короче 2 символов", true);
                }
                self.with_account(|a| a.name = text.clone());
                self.publish_profile(&text);
            }
            Target::FindFriends => {
                if text.chars().count() < 2 {
                    return self.toast("Введите хотя бы 2 символа", true);
                }
                if let Some(pk) = nostr::parse_public(&text).filter(|_| text.starts_with("npub1")) {
                    let name = self.social.profiles.get(&pk).map(|(n, _)| n.clone()).unwrap_or_else(|| format!("Игрок {}", &pk[..6]));
                    return self.show_players_for(&text, vec![(pk, name)]);
                }
                self.social.pending_find = Some(text);
                self.open(Modal::Message { title: "Найти друзей".into(), text: "Ищем игроков OTIUM…".into() });
                self.social.net.send(Cmd::Search);
            }
            _ => {}
        }
    }

    fn show_players(&mut self, list: Vec<(String, String)>) {
        let Some(q) = self.social.pending_find.take() else { return };
        let ql = q.to_lowercase();
        let me = self.social.me.clone().unwrap_or_default();
        let found: Vec<(String, String)> = list
            .into_iter()
            .filter(|(pk, name)| *pk != me && (name.to_lowercase().contains(&ql) || nostr::short_code(pk) == q.to_uppercase()))
            .take(30)
            .collect();
        self.show_players_for(&q, found);
    }

    fn show_players_for(&mut self, q: &str, found: Vec<(String, String)>) {
        let Some(a) = self.account() else { return };
        if found.is_empty() {
            return self.open(Modal::Message { title: "Найти друзей".into(), text: format!("По запросу «{q}» никого не нашли") });
        }
        let items = found
            .into_iter()
            .map(|(pk, name)| {
                let state = if a.is_friend(&pk) {
                    "уже в друзьях"
                } else if a.outgoing.contains(&pk) {
                    "заявка отправлена"
                } else {
                    "добавить"
                };
                (pk, name, state.to_string())
            })
            .collect();
        self.open(Modal::Players { items, focus: 0, scroll: 0.0 });
    }

    /// "добавить" on a search result.
    pub(super) fn request_friend(&mut self, pk: String, name: String) {
        let Some(a) = self.account() else { return };
        if a.is_friend(&pk) || a.outgoing.contains(&pk) {
            return;
        }
        self.social.profiles.insert(pk.clone(), (name.clone(), 0));
        if a.requests.iter().any(|r| r.pk == pk) {
            // they already asked us: accept instead
            self.add_friend(&pk, &name, true);
        } else {
            self.with_account(|a| a.outgoing.push(pk.clone()));
            self.send_ctrl(&pk, "fr");
        }
        self.toast(&format!("Заявка отправлена: {name}"), false);
        if let Some(Modal::Players { items, .. }) = self.modal.as_mut() {
            for it in items.iter_mut().filter(|it| it.0 == pk) {
                it.2 = "заявка отправлена".into();
            }
        }
    }

    pub(super) fn answer_request(&mut self, pk: String, accept: bool) {
        self.modal = None;
        if accept {
            let name = self.account().and_then(|a| a.requests.iter().find(|r| r.pk == pk).map(|r| r.name.clone())).unwrap_or_default();
            self.add_friend(&pk, &name, true);
            self.toast(&format!("{name} теперь в друзьях"), false);
        } else {
            self.with_account(|a| a.requests.retain(|r| r.pk != pk));
        }
    }

    // ---------- painting ----------

    pub(super) fn paint_social(&mut self, ui: &egui::Ui, rect: Rect, s: f32) {
        let p = ui.painter();
        p.rect_filled(rect, 0.0, Color32::from_rgba_unmultiplied(8, 10, 16, 245));
        p.text(pos2(96.0 * s, 64.0 * s), Align2::LEFT_CENTER, "Друзья и сообщения", semibold(32.0 * s), Color32::WHITE);

        let Some(a) = self.account() else {
            return self.paint_welcome(ui, rect, s);
        };
        let me = self.social.me.clone().unwrap_or_default();
        let items = self.items(&a);
        self.social.idx = self.social.idx.min(items.len().saturating_sub(1));

        // ---- left: me, find, requests, friends ----
        let left = Rect::from_min_max(pos2(96.0 * s, 120.0 * s), pos2(96.0 * s + 520.0 * s, rect.bottom() - 110.0 * s));
        let row_h = 76.0 * s;
        let mut y = left.top();
        let list_focus = self.social.zone == Zone::List && self.modal.is_none();
        let online = a.friends.iter().filter(|f| self.presence_of(&f.pk).0).count();
        let mut header_done = (false, false, false);
        let groups = a.groups();
        // scroll the list so the focused row stays visible
        let focus_y = self.social.idx as f32 * row_h + 80.0 * s;
        let offset = (focus_y - (left.height() - row_h * 2.0)).max(0.0);
        let clip = p.with_clip_rect(left);
        y -= offset;
        for (i, it) in items.iter().enumerate() {
            if matches!(it, Item::Request(_)) && !header_done.0 {
                header_done.0 = true;
                clip.text(pos2(left.left() + 8.0 * s, y + 18.0 * s), Align2::LEFT_CENTER, "Заявки", semibold(17.0 * s), white(0.5));
                y += 36.0 * s;
            }
            if matches!(it, Item::Group(_)) && !header_done.2 {
                header_done.2 = true;
                clip.text(pos2(left.left() + 8.0 * s, y + 18.0 * s), Align2::LEFT_CENTER, "Группы", semibold(17.0 * s), white(0.5));
                y += 36.0 * s;
            }
            if matches!(it, Item::Friend(_)) && !header_done.1 {
                header_done.1 = true;
                clip.text(pos2(left.left() + 8.0 * s, y + 18.0 * s), Align2::LEFT_CENTER, format!("Друзья · {online} в сети"), semibold(17.0 * s), white(0.5));
                y += 36.0 * s;
            }
            let r = Rect::from_min_size(pos2(left.left(), y), vec2(left.width(), row_h - 8.0 * s));
            y += row_h;
            if r.bottom() < left.top() || r.top() > left.bottom() {
                continue;
            }
            let f = list_focus && i == self.social.idx;
            let active = match it {
                Item::Friend(pk) => self.social.chat.as_deref() == Some(pk.as_str()),
                Item::Group(gid) => self.social.chat_is(&format!("g:{gid}")),
                _ => false,
            };
            if f {
                clip.rect_filled(r, CornerRadius::same((14.0 * s) as u8), Color32::WHITE);
            } else if active {
                clip.rect_filled(r, CornerRadius::same((14.0 * s) as u8), white(0.08));
            }
            let fg = if f { Color32::from_rgb(10, 12, 18) } else { Color32::WHITE };
            let av = pos2(r.left() + 40.0 * s, r.center().y);
            let tx = r.left() + 80.0 * s;
            match it {
                Item::Me => {
                    avatar(&clip, av, 24.0 * s, &me, &a.name);
                    clip.text(pos2(tx, r.center().y - 11.0 * s), Align2::LEFT_CENTER, &a.name, semibold(21.0 * s), fg);
                    clip.text(pos2(tx, r.center().y + 13.0 * s), Align2::LEFT_CENTER, format!("Мой профиль · код {}", nostr::short_code(&me)), font(16.0 * s), fg.gamma_multiply(0.6));
                }
                Item::Find => {
                    clip.circle_filled(av, 24.0 * s, if f { Color32::from_rgb(230, 234, 245) } else { white(0.1) });
                    clip.text(av, Align2::CENTER_CENTER, "+", bold(28.0 * s), fg);
                    clip.text(pos2(tx, r.center().y - 11.0 * s), Align2::LEFT_CENTER, "Найти друзей", semibold(21.0 * s), fg);
                    clip.text(pos2(tx, r.center().y + 13.0 * s), Align2::LEFT_CENTER, "по нику или коду", font(16.0 * s), fg.gamma_multiply(0.6));
                }
                Item::NewGroup => {
                    clip.circle_filled(av, 24.0 * s, if f { Color32::from_rgb(230, 234, 245) } else { white(0.1) });
                    clip.text(av, Align2::CENTER_CENTER, "👥", font(22.0 * s), fg);
                    clip.text(pos2(tx, r.center().y - 11.0 * s), Align2::LEFT_CENTER, "Создать группу", semibold(21.0 * s), fg);
                    clip.text(pos2(tx, r.center().y + 13.0 * s), Align2::LEFT_CENTER, "чат и голосовой для 3–5 друзей", font(16.0 * s), fg.gamma_multiply(0.6));
                }
                Item::Group(gid) => {
                    let Some(g) = groups.iter().find(|g| &g.id == gid) else { continue };
                    let title = g.title(&me);
                    clip.circle_filled(av, 24.0 * s, draw::name_color(&g.id));
                    clip.text(av, Align2::CENTER_CENTER, title.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default(), semibold(21.0 * s), Color32::WHITE);
                    clip.text(pos2(tx, r.center().y - 11.0 * s), Align2::LEFT_CENTER, &title, semibold(21.0 * s), fg);
                    let live = self.in_room(gid).len();
                    let (sub, col) = if live > 0 {
                        (format!("🔊 в голосовом: {live}"), GREEN)
                    } else {
                        (format!("{} {}", g.members.len(), crate::app::plural(g.members.len(), "участник", "участника", "участников")), fg.gamma_multiply(0.6))
                    };
                    clip.text(pos2(tx, r.center().y + 13.0 * s), Align2::LEFT_CENTER, sub, font(16.0 * s), col);
                    let u = a.group_unread(g, &me);
                    if u > 0 {
                        let b = pos2(r.right() - 26.0 * s, r.center().y);
                        clip.circle_filled(b, 13.0 * s, Color32::from_rgb(237, 66, 69));
                        clip.text(b, Align2::CENTER_CENTER, u.min(99).to_string(), semibold(14.0 * s), Color32::WHITE);
                    }
                }
                Item::Request(pk) => {
                    let name = self.display_name(&a, pk);
                    avatar(&clip, av, 24.0 * s, pk, &name);
                    clip.text(pos2(tx, r.center().y - 11.0 * s), Align2::LEFT_CENTER, &name, semibold(21.0 * s), fg);
                    clip.text(pos2(tx, r.center().y + 13.0 * s), Align2::LEFT_CENTER, "Заявка в друзья", font(16.0 * s), if f { BLUE } else { Color32::from_rgb(120, 160, 255) });
                }
                Item::Friend(pk) => {
                    let name = self.display_name(&a, pk);
                    let (on, text, game) = self.presence_of(pk);
                    avatar(&clip, av, 24.0 * s, pk, &name);
                    if on {
                        let dot = av + vec2(17.0 * s, 17.0 * s);
                        clip.circle_filled(dot, 7.0 * s, if f { Color32::WHITE } else { Color32::from_rgb(8, 10, 16) });
                        clip.circle_filled(dot, 5.0 * s, if game { BLUE } else { GREEN });
                    }
                    clip.text(pos2(tx, r.center().y - 11.0 * s), Align2::LEFT_CENTER, &name, semibold(21.0 * s), fg);
                    let sub_color = if game { if f { BLUE } else { Color32::from_rgb(120, 160, 255) } } else { fg.gamma_multiply(0.6) };
                    clip.text(pos2(tx, r.center().y + 13.0 * s), Align2::LEFT_CENTER, text, font(16.0 * s), sub_color);
                    let u = a.unread(pk, &me);
                    if u > 0 {
                        let b = pos2(r.right() - 26.0 * s, r.center().y);
                        clip.circle_filled(b, 13.0 * s, Color32::from_rgb(237, 66, 69));
                        clip.text(b, Align2::CENTER_CENTER, u.min(99).to_string(), semibold(14.0 * s), Color32::WHITE);
                    }
                }
            }
        }
        if a.friends.is_empty() {
            p.text(pos2(left.left() + 8.0 * s, y + 20.0 * s), Align2::LEFT_TOP, "Пока нет друзей — найдите их по нику", font(17.0 * s), white(0.5));
        }

        // ---- right: the chat ----
        let right = Rect::from_min_max(pos2(left.right() + 40.0 * s, 120.0 * s), pos2(rect.right() - 96.0 * s, rect.bottom() - 110.0 * s));
        p.rect_filled(right, CornerRadius::same((20.0 * s) as u8), Color32::from_rgb(18, 21, 30));
        let Some(pk) = self.social.chat.clone() else {
            p.text(right.center(), Align2::CENTER_CENTER, "Выберите друга, чтобы начать чат", font(22.0 * s), white(0.45));
            return;
        };
        if let Some(gid) = pk.strip_prefix("g:") {
            let gid = gid.to_string();
            return self.paint_group_chat(ui, right, s, &gid);
        }
        let name = self.display_name(&a, &pk);
        let (mut on, mut status, game) = self.presence_of(&pk);
        if self.calls.call.pk == pk && self.calls.call.state == super::calls::CallState::Active {
            on = true;
            status = format!("● В звонке · Y — меню звонка");
        }
        let head = Rect::from_min_size(right.min, vec2(right.width(), 84.0 * s));
        avatar(p, pos2(head.left() + 48.0 * s, head.center().y), 26.0 * s, &pk, &name);
        p.text(pos2(head.left() + 90.0 * s, head.center().y - 12.0 * s), Align2::LEFT_CENTER, &name, semibold(23.0 * s), Color32::WHITE);
        let sc = if game { Color32::from_rgb(120, 160, 255) } else if on { GREEN } else { white(0.5) };
        p.text(pos2(head.left() + 90.0 * s, head.center().y + 14.0 * s), Align2::LEFT_CENTER, status, font(16.0 * s), sc);
        p.line_segment([pos2(right.left(), head.bottom()), pos2(right.right(), head.bottom())], Stroke::new(1.0_f32, white(0.06)));

        // input bar
        let chat_focus = self.social.zone == Zone::Chat && self.modal.is_none();
        let input = Rect::from_min_max(pos2(right.left() + 20.0 * s, right.bottom() - 84.0 * s), pos2(right.right() - 20.0 * s, right.bottom() - 20.0 * s));
        if chat_focus {
            draw::glow(p, input, 16.0 * s, Color32::WHITE, 12.0 * s, 0.4);
        }
        p.rect_filled(input, CornerRadius::same((16.0 * s) as u8), if chat_focus { white(0.14) } else { white(0.06) });
        p.text(pos2(input.left() + 22.0 * s, input.center().y), Align2::LEFT_CENTER, format!("Сообщение для {name} · Y — позвонить"), font(19.0 * s), white(0.45));

        // messages, newest at the bottom, grouped like Discord
        let msgs = a.history.get(&pk).cloned().unwrap_or_default();
        let area = Rect::from_min_max(pos2(right.left() + 24.0 * s, head.bottom() + 12.0 * s), pos2(right.right() - 24.0 * s, input.top() - 14.0 * s));
        if msgs.is_empty() {
            p.text(area.center(), Align2::CENTER_CENTER, format!("Это начало переписки с {name}. Скажите привет 👋"), font(20.0 * s), white(0.5));
            return;
        }
        let my_name = a.name.clone();
        let other = name.clone();
        let me2 = me.clone();
        let mut scroll = self.social.chat_scroll;
        paint_messages(p, area, s, &msgs, &me, &mut scroll, &move |pk: &str| if pk == me2 { my_name.clone() } else { other.clone() });
        self.social.chat_scroll = scroll;
    }

    fn paint_welcome(&self, ui: &egui::Ui, rect: Rect, s: f32) {
        let p = ui.painter();
        let c = rect.center();
        p.text(c - vec2(0.0, 140.0 * s), Align2::CENTER_CENTER, "Аккаунт OTIUM", bold(40.0 * s), Color32::WHITE);
        p.text(
            c - vec2(0.0, 84.0 * s),
            Align2::CENTER_CENTER,
            "Создайте аккаунт, чтобы добавлять друзей и переписываться прямо в OTIUM. Регистрация не нужна.",
            font(20.0 * s),
            white(0.6),
        );
        for (i, label) in ["Создать аккаунт", "У меня есть ключ входа"].iter().enumerate() {
            let r = Rect::from_center_size(c + vec2(0.0, i as f32 * 84.0 * s), vec2(420.0 * s, 64.0 * s));
            let f = self.social.idx == i && self.modal.is_none();
            if f {
                draw::glow(p, r, 32.0 * s, Color32::WHITE, 14.0 * s, 0.5);
            }
            p.rect_filled(r, CornerRadius::same((32.0 * s) as u8), if f { Color32::WHITE } else if i == 0 { BLUE } else { white(0.1) });
            p.text(r.center(), Align2::CENTER_CENTER, *label, semibold(22.0 * s), if f { Color32::from_rgb(10, 12, 18) } else { Color32::WHITE });
        }
    }
}

/// A round avatar with the first letter of the name, coloured by the key.
fn avatar(p: &egui::Painter, c: egui::Pos2, r: f32, key: &str, name: &str) {
    p.circle_filled(c, r, draw::name_color(if key.is_empty() { name } else { key }));
    let letter = name.chars().next().map(|ch| ch.to_uppercase().to_string()).unwrap_or_else(|| "?".into());
    p.text(c, Align2::CENTER_CENTER, letter, semibold(r * 0.9), Color32::WHITE);
}

/// Messages, newest at the bottom, grouped like Discord: one header per run of messages from the
/// same person within 5 minutes. `scroll` is in lines up from the bottom.
pub(super) fn paint_messages(p: &egui::Painter, area: Rect, s: f32, msgs: &[Message], me: &str, scroll: &mut f32, name_of: &dyn Fn(&str) -> String) {
    let clip = p.with_clip_rect(area);
    let line_h = 28.0 * s;
    let mut blocks: Vec<(String, i64, Vec<String>)> = Vec::new();
    for m in msgs {
        let new_group = blocks.last().is_none_or(|b: &(String, i64, Vec<String>)| b.0 != m.from || m.at - b.1 > 5 * 60_000);
        if new_group {
            blocks.push((m.from.clone(), m.at, vec![m.text.clone()]));
        } else if let Some(b) = blocks.last_mut() {
            b.1 = m.at;
            b.2.push(m.text.clone());
        }
    }
    *scroll = scroll.min(10_000.0);
    let mut y = area.bottom() + *scroll * line_h;
    let wrap = area.width() - 70.0 * s;
    for (from, at, lines) in blocks.iter().rev() {
        let galleys: Vec<_> = lines.iter().map(|t| p.layout(t.clone(), font(19.0 * s), white(0.92), wrap)).collect();
        let h: f32 = galleys.iter().map(|g| g.size().y + 4.0 * s).sum::<f32>() + 30.0 * s;
        y -= h + 14.0 * s;
        if y > area.bottom() || y + h < area.top() {
            continue;
        }
        let mine = from == me;
        let who = name_of(from);
        avatar(&clip, pos2(area.left() + 22.0 * s, y + 22.0 * s), 20.0 * s, from, &who);
        let time = chrono::TimeZone::timestamp_millis_opt(&chrono::Local, *at).single().map(|d| d.format("%H:%M").to_string()).unwrap_or_default();
        let date = crate::app::fmt_date(*at as f64);
        let when = if date.starts_with("сегодня") { time } else { date };
        let tx = area.left() + 56.0 * s;
        let nr = clip.text(pos2(tx, y + 4.0 * s), Align2::LEFT_TOP, &who, semibold(18.0 * s), if mine { Color32::from_rgb(150, 180, 255) } else { Color32::WHITE });
        clip.text(pos2(nr.right() + 10.0 * s, y + 7.0 * s), Align2::LEFT_TOP, when, font(14.0 * s), white(0.4));
        let mut ly = y + 30.0 * s;
        for g in galleys {
            let gh = g.size().y;
            clip.galley(pos2(tx, ly), g, Color32::WHITE);
            ly += gh + 4.0 * s;
        }
    }
}

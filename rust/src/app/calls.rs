//! Voice calls between friends and group chats with a voice room, speaking the Electron build's
//! protocol: control messages {"pl":"ps5l","t":…} inside the encrypted chat carry offers, answers
//! and group events; the audio goes straight between PCs over WebRTC (see `crate::voice`).

use super::App;
use crate::account::{Group, Member, Message};
use crate::draw::{self, bold, font, semibold, white};
use crate::input::Nav;
use crate::modal::{Modal, SetItem, SetKey, SetMenu};
use crate::sound::{play, Sfx};
use crate::voice::{Cmd as VCmd, Event as VEvent, Voice};
use egui::{pos2, vec2, Align2, Color32, CornerRadius, Rect, Stroke};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::time::{Duration, Instant};

const CTRL: &str = "ps5l";
pub const CALL_CTRL: [&str; 6] = ["call", "ans", "decl", "busy", "end", "move"];
pub const GROUP_CTRL: [&str; 11] = ["ginv", "gmsg", "gleave", "vjoin", "vhere", "voff", "vans", "vleave", "vping", "vring", "vdecl"];
pub const MAX_GROUP: usize = 5;
const RING_TIMEOUT: Duration = Duration::from_secs(40);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(25);
const NAT_TEXT: &str = "Не удалось соединиться напрямую. Чаще всего так бывает, когда у обоих провайдер раздаёт адреса «за общим NAT».";
const GREEN: Color32 = Color32::from_rgb(67, 181, 129);
const RED: Color32 = Color32::from_rgb(237, 66, 69);

#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum CallState {
    #[default]
    Idle,
    Incoming,
    Outgoing,
    Connecting,
    Active,
}

#[derive(Default)]
pub struct Call {
    pub state: CallState,
    pub pk: String,
    pub id: String,
    offer: Option<String>,
    pub started: Option<Instant>,
    pub ring_started: Option<Instant>,
    pub muted: bool,
    pub deafened: bool,
    muted_before: bool,
    pub remote_muted: bool,
    pub remote_deafened: bool,
    deadline: Option<Instant>,
}

#[derive(Default, Clone)]
pub struct RoomPeer {
    pub connected: bool,
    pub muted: bool,
    pub deafened: bool,
}

pub struct Room {
    pub gid: String,
    pub joined: Instant,
    pub peers: HashMap<String, RoomPeer>,
    pub muted: bool,
    pub deafened: bool,
    muted_before: bool,
    ping_at: Instant,
}

pub struct GroupRing {
    pub gid: String,
    pub from: String,
    until: Instant,
}

/// What the friend picker was opened for.
#[derive(Clone, Debug)]
pub enum PickFor {
    NewGroup,
    Invite(String),
    AddToCall,
    SendClip(std::path::PathBuf),
}

pub struct Calls {
    pub voice: Voice,
    pub call: Call,
    pub room: Option<Room>,
    /// gid -> who was last seen in its voice room
    roster: HashMap<String, HashMap<String, Instant>>,
    pub ring: Option<GroupRing>,
    pub speaking: HashMap<String, bool>,
    ring_sound_at: Option<Instant>,
    /// after a 1:1 call moved into a group: join it once the invite arrives
    follow: Option<(String, Instant)>,
    /// friends picked for a new group, waiting for its name
    pub pending_group: Vec<String>,
    /// group a menu was opened for
    pub menu_gid: Option<String>,
}

impl Calls {
    pub fn new(bus: crate::watch::Bus) -> Self {
        let voice = Voice::start(move |e| bus.send(crate::watch::Event::Voice(e)));
        Self {
            voice,
            call: Call::default(),
            room: None,
            roster: HashMap::new(),
            ring: None,
            speaking: HashMap::new(),
            ring_sound_at: None,
            follow: None,
            pending_group: Vec::new(),
            menu_gid: None,
        }
    }

    pub fn in_call(&self) -> bool {
        self.call.state != CallState::Idle
    }

    /// The call card (incoming / outgoing / connecting / group ring) takes the gamepad.
    pub fn card_open(&self) -> bool {
        matches!(self.call.state, CallState::Incoming | CallState::Outgoing | CallState::Connecting) || self.ring.is_some()
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

fn short_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

pub(super) fn mmss(d: Duration) -> String {
    let s = d.as_secs();
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{:02}:{:02}", s / 60, s % 60)
    }
}

impl App {
    pub(super) fn me_pk(&self) -> String {
        self.social.me_pk().unwrap_or_default()
    }

    fn my_name(&self) -> String {
        self.account().map(|a| a.name).unwrap_or_default()
    }

    fn groups(&self) -> Vec<Group> {
        self.account().map(|a| a.groups()).unwrap_or_default()
    }

    pub(super) fn group(&self, gid: &str) -> Option<Group> {
        self.groups().into_iter().find(|g| g.id == gid)
    }

    fn save_groups(&mut self, groups: Vec<Group>) {
        self.with_account(|a| a.set_groups(&groups));
    }

    fn update_group(&mut self, gid: &str, f: impl FnOnce(&mut Group)) {
        let mut gs = self.groups();
        if let Some(g) = gs.iter_mut().find(|g| g.id == gid) {
            f(g);
        }
        self.save_groups(gs);
    }

    pub(super) fn member_name(&self, g: &Group, pk: &str) -> String {
        if pk == self.me_pk() {
            return self.my_name();
        }
        g.member_name(pk).map(str::to_string).unwrap_or_else(|| self.account().map(|a| self.display_name(&a, pk)).unwrap_or_default())
    }

    fn send_call(&self, pk: &str, t: &str, extra: Value) {
        let mut body = json!({ "pl": CTRL, "t": t, "n": self.my_name() });
        if let (Some(b), Some(e)) = (body.as_object_mut(), extra.as_object()) {
            b.extend(e.clone());
        }
        self.social.send_raw(pk, body.to_string());
    }

    /// One copy per member (or just `only`).
    fn send_group(&self, g: &Group, t: &str, extra: Value, only: Option<&str>) {
        let me = self.me_pk();
        let mut body = json!({ "pl": CTRL, "t": t, "gid": g.id, "n": self.my_name() });
        if let (Some(b), Some(e)) = (body.as_object_mut(), extra.as_object()) {
            b.extend(e.clone());
        }
        let text = body.to_string();
        for m in &g.members {
            if m.pk != me && only.is_none_or(|o| o == m.pk) {
                self.social.send_raw(&m.pk, text.clone());
            }
        }
    }

    fn voice_settings(&self) -> (f32, bool) {
        let v = self.lib.settings.get("voice");
        let vol = v.and_then(|v| v["volume"].as_f64()).unwrap_or(100.0) as f32;
        let noise = v.and_then(|v| v["noise"].as_bool()).unwrap_or(true);
        (vol, noise)
    }

    fn apply_voice_settings(&self) {
        let (vol, noise) = self.voice_settings();
        self.calls.voice.send(VCmd::Volume(vol / 100.0));
        self.calls.voice.send(VCmd::Noise(noise));
        self.calls.voice.send(VCmd::Mic(self.mic_setting()));
        self.calls.voice.send(VCmd::Gate(self.vad_setting().0.then_some(self.vad_setting().1)));
    }

    /// Voice activation (Rust build only): `voice.vad` on/off and `voice.vadDb`, the threshold.
    fn vad_setting(&self) -> (bool, f32) {
        let v = self.lib.settings.get("voice");
        let on = v.and_then(|v| v["vad"].as_bool()).unwrap_or(false);
        let db = v.and_then(|v| v["vadDb"].as_f64()).unwrap_or(VAD_DEFAULT_DB as f64) as f32;
        (on, db.clamp(VAD_MIN_DB, VAD_MAX_DB))
    }

    fn set_vad(&mut self, on: bool, db: f32) {
        let db = db.clamp(VAD_MIN_DB, VAD_MAX_DB);
        let mut voice = self.lib.settings.get("voice").cloned().unwrap_or_else(|| json!({ "volume": 100, "micId": "", "noise": true }));
        voice["vad"] = json!(on);
        voice["vadDb"] = json!(db as i64);
        self.lib.settings.insert("voice".into(), voice);
        self.save();
        self.calls.voice.send(VCmd::Gate(on.then_some(db)));
    }

    /// The voice activation row: threshold and how loud the mic is right now, so the user can
    /// set the threshold between their background and their voice.
    fn vad_label(&self) -> String {
        let (on, db) = self.vad_setting();
        let now = self.calls.voice.mic_db();
        let now = if now > -99.0 { format!(" · сейчас {}", fmt_db(now)) } else { String::new() };
        if on {
            format!("порог {} ◀ ▶{now}", fmt_db(db))
        } else {
            format!("Выкл{now}")
        }
    }

    /// Keeps the live mic level on the voice activation row fresh while the call menu is open.
    pub(super) fn tick_call_menu(&mut self, ctx: &egui::Context) {
        if !matches!(&self.modal, Some(Modal::Settings { menu: SetMenu::Call, .. })) {
            return;
        }
        let label = self.vad_label();
        if let Some(Modal::Settings { items, .. }) = &mut self.modal {
            if let Some(i) = items.iter_mut().find(|i| i.key == SetKey::CallVad) {
                i.value = label;
            }
        }
        ctx.request_repaint_after(Duration::from_millis(100));
    }

    /// `voice.micId`: a device name here; the Electron build stores Chromium ids there, which
    /// match no name, so they mean "default" (and Electron falls back the same way for ours).
    fn mic_setting(&self) -> String {
        self.lib.settings.get("voice").and_then(|v| v["micId"].as_str()).unwrap_or_default().to_string()
    }

    /// Next microphone in the list ("" = Windows default first).
    fn cycle_mic(&mut self) {
        let mut list = vec![String::new()];
        list.extend(crate::voice::mics());
        let cur = self.mic_setting();
        let i = list.iter().position(|m| *m == cur).unwrap_or(0);
        let next = list[(i + 1) % list.len()].clone();
        let mut voice = self.lib.settings.get("voice").cloned().unwrap_or_else(|| json!({ "volume": 100, "micId": "", "noise": true }));
        voice["micId"] = json!(next);
        self.lib.settings.insert("voice".into(), voice);
        self.save();
        self.calls.voice.send(VCmd::Mic(next));
    }

    // ---------- 1:1 calls ----------

    pub(super) fn start_call(&mut self, pk: &str) {
        if self.social.me_pk().is_none() {
            return self.toast("Сначала создайте аккаунт в «Друзья и сообщения»", true);
        }
        if self.calls.in_call() {
            return self.toast("Вы уже в звонке", true);
        }
        if self.calls.room.is_some() {
            self.leave_room();
        }
        self.apply_voice_settings();
        let c = &mut self.calls.call;
        *c = Call { state: CallState::Outgoing, pk: pk.into(), id: short_id(), ring_started: Some(Instant::now()), ..Default::default() };
        c.deadline = Some(Instant::now() + RING_TIMEOUT);
        self.calls.ring_sound_at = Some(Instant::now());
        self.calls.voice.send(VCmd::Offer(pk.into()));
    }

    pub(super) fn accept_call(&mut self) {
        if self.calls.call.state != CallState::Incoming {
            return;
        }
        let Some(offer) = self.calls.call.offer.take() else { return };
        self.apply_voice_settings();
        self.calls.call.state = CallState::Connecting;
        self.calls.call.deadline = Some(Instant::now() + CONNECT_TIMEOUT);
        self.calls.ring_sound_at = None;
        let pk = self.calls.call.pk.clone();
        self.calls.voice.send(VCmd::Answer(pk, offer));
    }

    pub(super) fn decline_call(&mut self) {
        if self.calls.call.state != CallState::Incoming {
            return;
        }
        let (pk, id) = (self.calls.call.pk.clone(), self.calls.call.id.clone());
        self.send_call(&pk, "decl", json!({ "id": id }));
        self.end_call(None);
    }

    pub(super) fn hang_up(&mut self) {
        if !self.calls.in_call() {
            return;
        }
        let (pk, id) = (self.calls.call.pk.clone(), self.calls.call.id.clone());
        self.send_call(&pk, "end", json!({ "id": id }));
        self.end_call(Some("Звонок завершён".into()));
    }

    fn end_call(&mut self, msg: Option<String>) {
        let had = self.calls.call.state != CallState::Idle;
        let pk = self.calls.call.pk.clone();
        self.calls.call = Call::default();
        self.calls.ring_sound_at = None;
        self.calls.voice.send(VCmd::Close(pk));
        if matches!(self.modal, Some(Modal::Settings { menu: SetMenu::Call, .. })) {
            self.modal = None;
        }
        if had {
            play(Sfx::End);
        }
        if let Some(m) = msg {
            self.toast(&m, false);
        }
    }

    pub(super) fn toggle_mute(&mut self) {
        if let Some(r) = self.calls.room.as_mut() {
            r.muted = !r.muted;
            if !r.muted && r.deafened {
                r.deafened = false;
                self.calls.voice.send(VCmd::Deafened(false));
            }
            let m = r.muted;
            self.calls.voice.send(VCmd::Muted(m));
            play(if m { Sfx::MicOff } else { Sfx::MicOn });
            return;
        }
        let c = &mut self.calls.call;
        if c.state == CallState::Idle || c.state == CallState::Incoming {
            return;
        }
        c.muted = !c.muted;
        // un-muting the mic also brings the sound back, like Discord
        if !c.muted && c.deafened {
            c.deafened = false;
            self.calls.voice.send(VCmd::Deafened(false));
        }
        let m = c.muted;
        self.calls.voice.send(VCmd::Muted(m));
        play(if m { Sfx::MicOff } else { Sfx::MicOn });
    }

    /// "Выключить звук": stop hearing others; also mutes the mic, like Discord's deafen.
    pub(super) fn toggle_deafen(&mut self) {
        let (muted, deafened, before) = if let Some(r) = self.calls.room.as_mut() {
            (&mut r.muted, &mut r.deafened, &mut r.muted_before)
        } else if self.calls.call.state == CallState::Active || self.calls.call.state == CallState::Connecting {
            let c = &mut self.calls.call;
            (&mut c.muted, &mut c.deafened, &mut c.muted_before)
        } else {
            return;
        };
        *deafened = !*deafened;
        if *deafened {
            *before = *muted;
            *muted = true;
        } else {
            *muted = *before;
        }
        let (m, d) = (*muted, *deafened);
        self.calls.voice.send(VCmd::Muted(m));
        self.calls.voice.send(VCmd::Deafened(d));
        play(if d { Sfx::MicOff } else { Sfx::MicOn });
    }

    fn set_volume(&mut self, v: f32) {
        let v = (v / 5.0).round() * 5.0;
        let v = v.clamp(0.0, 200.0);
        let mut voice = self.lib.settings.get("voice").cloned().unwrap_or_else(|| json!({ "volume": 100, "micId": "", "noise": true }));
        voice["volume"] = json!(v as i64);
        self.lib.settings.insert("voice".into(), voice);
        self.save();
        self.calls.voice.send(VCmd::Volume(v / 100.0));
    }

    fn toggle_noise(&mut self) {
        let (_, noise) = self.voice_settings();
        let mut voice = self.lib.settings.get("voice").cloned().unwrap_or_else(|| json!({ "volume": 100, "micId": "", "noise": true }));
        voice["noise"] = json!(!noise);
        self.lib.settings.insert("voice".into(), voice);
        self.save();
        self.calls.voice.send(VCmd::Noise(!noise));
    }

    // ---------- incoming signals ----------

    pub(super) fn on_call_ctrl(&mut self, from: &str, c: &Value, at_ms: i64) {
        // stale signals replayed from history mean nothing
        if now_ms() - at_ms > 60_000 {
            return;
        }
        let t = c["t"].as_str().unwrap_or("");
        let id = c["id"].as_str().unwrap_or("").to_string();
        let Some(a) = self.account() else { return };
        if t == "call" {
            if !a.is_friend(from) {
                return;
            }
            if self.calls.in_call() || self.calls.room.is_some() {
                return self.send_call(from, "busy", json!({ "id": id }));
            }
            let sdp = c["sdp"].as_str().unwrap_or("").to_string();
            self.calls.call = Call { state: CallState::Incoming, pk: from.into(), id, offer: Some(sdp), ring_started: Some(Instant::now()), ..Default::default() };
            self.calls.call.deadline = Some(Instant::now() + RING_TIMEOUT);
            self.calls.ring_sound_at = Some(Instant::now());
            // over a game: bring OTIUM up so the call can be answered
            if self.away_for_game {
                self.show_window();
            }
            return;
        }
        if from != self.calls.call.pk || id != self.calls.call.id {
            return;
        }
        let name = self.display_name(&a, from);
        match t {
            "ans" if self.calls.call.state == CallState::Outgoing => {
                self.calls.call.state = CallState::Connecting;
                self.calls.call.deadline = Some(Instant::now() + CONNECT_TIMEOUT);
                self.calls.ring_sound_at = None;
                let sdp = c["sdp"].as_str().unwrap_or("").to_string();
                self.calls.voice.send(VCmd::Accepted(from.into(), sdp));
            }
            "move" => {
                // the other side added someone: the call continues in a group voice room
                self.end_call(None);
                if let Some(gid) = c["gid"].as_str() {
                    self.calls.follow = Some((gid.into(), Instant::now() + Duration::from_secs(5)));
                }
            }
            "decl" => self.end_call(Some(format!("{name} отклонил(а) звонок"))),
            "busy" => self.end_call(Some(format!("{name} сейчас в другом звонке"))),
            "end" => {
                let missed = self.calls.call.state == CallState::Incoming;
                self.end_call(Some(if missed { format!("Пропущенный звонок от {name}") } else { "Звонок завершён".into() }));
            }
            _ => {}
        }
    }

    pub(super) fn on_voice(&mut self, e: VEvent) {
        match e {
            VEvent::Sdp { peer, sdp, offer } => {
                if self.calls.call.pk == peer && self.calls.call.state != CallState::Idle {
                    let id = self.calls.call.id.clone();
                    self.send_call(&peer, if offer { "call" } else { "ans" }, json!({ "id": id, "sdp": sdp }));
                } else if let Some(g) = self.calls.room.as_ref().and_then(|r| self.group(&r.gid)) {
                    self.send_group(&g, if offer { "voff" } else { "vans" }, json!({ "sdp": sdp }), Some(&peer));
                }
            }
            VEvent::Connected(peer) => {
                if self.calls.call.pk == peer && self.calls.call.state != CallState::Active {
                    self.calls.call.state = CallState::Active;
                    self.calls.call.started = Some(Instant::now());
                    self.calls.call.deadline = None;
                    self.calls.ring_sound_at = None;
                    play(Sfx::Connect);
                } else if let Some(p) = self.calls.room.as_mut().and_then(|r| r.peers.get_mut(&peer)) {
                    p.connected = true;
                    play(Sfx::MicOn);
                }
            }
            VEvent::Failed(peer) | VEvent::Closed(peer) => {
                if self.calls.call.pk == peer && matches!(self.calls.call.state, CallState::Connecting | CallState::Active | CallState::Outgoing) {
                    if self.calls.call.state == CallState::Active {
                        self.hang_up();
                    } else {
                        self.toast(NAT_TEXT, true);
                        self.hang_up();
                    }
                } else if let Some(r) = self.calls.room.as_mut() {
                    if let Some(p) = r.peers.remove(&peer) {
                        self.calls.voice.send(VCmd::Close(peer.clone()));
                        if p.connected {
                            play(Sfx::MicOff);
                        } else {
                            self.toast(NAT_TEXT, true);
                        }
                    }
                }
            }
            VEvent::RemoteState { peer, muted, deafened } => {
                if self.calls.call.pk == peer {
                    self.calls.call.remote_muted = muted;
                    self.calls.call.remote_deafened = deafened;
                } else if let Some(p) = self.calls.room.as_mut().and_then(|r| r.peers.get_mut(&peer)) {
                    p.muted = muted;
                    p.deafened = deafened;
                }
            }
            VEvent::Speaking(map) => self.calls.speaking = map,
            VEvent::MicError(e) => {
                self.toast(&e, true);
                if self.calls.call.state == CallState::Connecting && self.calls.call.offer.is_none() {
                    self.hang_up();
                } else if self.calls.in_call() {
                    self.hang_up();
                }
            }
        }
    }

    /// Ring sounds, timeouts, room pings; every frame.
    pub(super) fn calls_tick(&mut self) {
        let now = Instant::now();
        if let Some(t) = self.calls.ring_sound_at {
            if now >= t {
                let incoming = self.calls.call.state == CallState::Incoming || self.calls.ring.is_some();
                play(if incoming { Sfx::Ring } else { Sfx::Ringback });
                self.calls.ring_sound_at = Some(now + if incoming { Duration::from_millis(2400) } else { Duration::from_millis(3200) });
            }
        }
        if self.calls.call.deadline.is_some_and(|d| now >= d) {
            let name = self.account().map(|a| self.display_name(&a, &self.calls.call.pk)).unwrap_or_default();
            match self.calls.call.state {
                CallState::Outgoing => {
                    self.toast(&format!("{name} не отвечает"), false);
                    self.hang_up();
                }
                CallState::Incoming => self.end_call(Some(format!("Пропущенный звонок от {name}"))),
                CallState::Connecting => {
                    self.toast(NAT_TEXT, true);
                    self.hang_up();
                }
                _ => self.calls.call.deadline = None,
            }
        }
        if let Some(r) = &self.calls.room {
            if now >= r.ping_at {
                let gid = r.gid.clone();
                if let Some(g) = self.group(&gid) {
                    self.send_group(&g, "vping", json!({}), None);
                }
                if let Some(r) = self.calls.room.as_mut() {
                    r.ping_at = now + Duration::from_secs(60);
                }
            }
        }
        if self.calls.ring.as_ref().is_some_and(|r| now >= r.until) {
            self.stop_group_ring();
        }
        if let Some((gid, until)) = self.calls.follow.clone() {
            if self.group(&gid).is_some() {
                self.calls.follow = None;
                self.join_room(&gid);
            } else if now >= until {
                self.calls.follow = None;
            }
        }
    }

    // ---------- groups ----------

    pub(super) fn create_group(&mut self, name: &str, pks: &[String]) -> Option<String> {
        let a = self.account()?;
        let me = self.me_pk();
        let mut members = vec![Member { pk: me, name: a.name.clone() }];
        members.extend(pks.iter().map(|pk| Member { pk: pk.clone(), name: self.display_name(&a, pk) }));
        let g = Group { id: short_id()[..16].to_string(), name: name.trim().into(), owner: self.me_pk(), created: now_ms(), members, history: vec![], extra: Default::default() };
        let mut gs = self.groups();
        gs.push(g.clone());
        self.save_groups(gs);
        self.send_group(&g, "ginv", json!({ "name": g.name, "owner": g.owner, "members": g.members }), None);
        Some(g.id)
    }

    fn invite(&mut self, gid: &str, pks: &[String]) {
        let Some(a) = self.account() else { return };
        let names: Vec<(String, String)> = pks.iter().map(|pk| (pk.clone(), self.display_name(&a, pk))).collect();
        self.update_group(gid, |g| {
            for (pk, name) in names {
                if !g.is_member(&pk) && g.members.len() < MAX_GROUP {
                    g.members.push(Member { pk, name });
                }
            }
        });
        if let Some(g) = self.group(gid) {
            self.send_group(&g, "ginv", json!({ "name": g.name, "owner": g.owner, "members": g.members }), None);
        }
    }

    fn leave_group(&mut self, gid: &str) {
        if self.calls.room.as_ref().is_some_and(|r| r.gid == gid) {
            self.leave_room();
        }
        if let Some(g) = self.group(gid) {
            self.send_group(&g, "gleave", json!({}), None);
        }
        let gs: Vec<Group> = self.groups().into_iter().filter(|g| g.id != gid).collect();
        self.save_groups(gs);
        self.social.close_chat_if(&format!("g:{gid}"));
    }

    pub(super) fn send_group_message(&mut self, gid: &str, text: &str) {
        let mid = short_id()[..12].to_string();
        let me = self.me_pk();
        let text_owned = text.to_string();
        let mid2 = mid.clone();
        self.update_group(gid, |g| {
            g.history.push(Message { id: mid2, from: me, text: text_owned, at: now_ms() });
            let extra = g.history.len().saturating_sub(400);
            g.history.drain(..extra);
        });
        if let Some(g) = self.group(gid) {
            self.send_group(&g, "gmsg", json!({ "text": text, "mid": mid }), None);
        }
        self.mark_read(&format!("g:{gid}"));
    }

    pub(super) fn on_group_ctrl(&mut self, from: &str, c: &Value, at_ms: i64) {
        let age = now_ms() - at_ms;
        let t = c["t"].as_str().unwrap_or("");
        let gid = c["gid"].as_str().unwrap_or("").to_string();
        let me = self.me_pk();
        let g = self.group(&gid);

        if t == "ginv" {
            let members: Vec<Member> = serde_json::from_value(c["members"].clone()).unwrap_or_default();
            let mine = members.iter().any(|m| m.pk == me);
            if !members.iter().any(|m| m.pk == from) {
                return;
            }
            match g {
                None if mine => {
                    let ng = Group {
                        id: gid.clone(),
                        name: c["name"].as_str().unwrap_or("").into(),
                        owner: c["owner"].as_str().unwrap_or("").into(),
                        created: at_ms,
                        members,
                        history: vec![],
                        extra: Default::default(),
                    };
                    let title = ng.title(&me);
                    let mut gs = self.groups();
                    gs.push(ng);
                    self.save_groups(gs);
                    if age < 10 * 60_000 {
                        let who = c["n"].as_str().filter(|n| !n.is_empty()).unwrap_or("Друг");
                        self.toast(&format!("{who} добавил(а) вас в группу «{title}»"), false);
                    }
                }
                Some(g) if g.is_member(from) => {
                    if !mine {
                        // we were removed
                        let gs: Vec<Group> = self.groups().into_iter().filter(|x| x.id != gid).collect();
                        self.save_groups(gs);
                        return;
                    }
                    let name = c["name"].as_str().unwrap_or("").to_string();
                    self.update_group(&gid, |g| {
                        g.name = name;
                        g.members = members;
                    });
                }
                _ => {}
            }
            return;
        }
        let Some(g) = g.filter(|g| g.is_member(from)) else { return };

        match t {
            "gmsg" => {
                let mid = c["mid"].as_str().unwrap_or("").to_string();
                if g.history.iter().any(|m| m.id == mid) {
                    return;
                }
                let text = c["text"].as_str().unwrap_or("").to_string();
                let msg = Message { id: mid, from: from.into(), text: text.clone(), at: at_ms };
                self.update_group(&gid, |g| {
                    g.history.push(msg);
                    g.history.sort_by_key(|m| m.at);
                    let extra = g.history.len().saturating_sub(400);
                    g.history.drain(..extra);
                });
                let viewing = self.social.open && self.social.chat_is(&format!("g:{gid}"));
                if viewing {
                    self.mark_read(&format!("g:{gid}"));
                } else if age < 5 * 60_000 {
                    let who = self.member_name(&g, from);
                    let short: String = text.chars().take(120).collect();
                    self.notify(&format!("{who} · {}", g.title(&me)), &short, (g.id.clone(), g.title(&me)));
                    if self.sounds_on() {
                        play(Sfx::Ok);
                    }
                }
                return;
            }
            "gleave" => {
                self.update_group(&gid, |g| g.members.retain(|m| m.pk != from));
                self.close_room_peer(from);
                if let Some(r) = self.calls.roster.get_mut(&gid) {
                    r.remove(from);
                }
                return;
            }
            _ => {}
        }

        // voice room signals only mean something while fresh
        if age > 90_000 {
            return;
        }
        let in_room = self.calls.room.as_ref().is_some_and(|r| r.gid == gid);
        match t {
            "vjoin" | "vhere" | "vping" => {
                self.calls.roster.entry(gid.clone()).or_default().insert(from.into(), Instant::now());
                if in_room {
                    if t == "vjoin" {
                        self.send_group(&g, "vhere", json!({}), Some(from));
                    }
                    // to avoid both sides calling at once, the smaller key makes the offer
                    let have = self.calls.room.as_ref().is_some_and(|r| r.peers.contains_key(from));
                    if t != "vping" && me.as_str() < from && !have {
                        self.connect_room_peer(from);
                    }
                }
            }
            "vleave" => {
                if let Some(r) = self.calls.roster.get_mut(&gid) {
                    r.remove(from);
                }
                self.close_room_peer(from);
            }
            "voff" if in_room => {
                let sdp = c["sdp"].as_str().unwrap_or("").to_string();
                if let Some(r) = self.calls.room.as_mut() {
                    r.peers.insert(from.into(), RoomPeer::default());
                }
                self.calls.voice.send(VCmd::Answer(from.into(), sdp));
            }
            "vans" if in_room => {
                let sdp = c["sdp"].as_str().unwrap_or("").to_string();
                self.calls.voice.send(VCmd::Accepted(from.into(), sdp));
            }
            "vring" => {
                self.calls.roster.entry(gid.clone()).or_default().insert(from.into(), Instant::now());
                if !in_room && !self.calls.in_call() && !self.calls.card_open() {
                    self.calls.ring = Some(GroupRing { gid: gid.clone(), from: from.into(), until: Instant::now() + Duration::from_secs(35) });
                    self.calls.ring_sound_at = Some(Instant::now());
                    if self.away_for_game {
                        self.show_window();
                    }
                }
            }
            _ => {}
        }
    }

    /// Who is in a group's voice room, as far as we know (anyone silent for 3 minutes is gone).
    pub(super) fn in_room(&self, gid: &str) -> Vec<String> {
        let mut v: Vec<String> = self
            .calls
            .roster
            .get(gid)
            .map(|r| r.iter().filter(|(_, t)| t.elapsed() < Duration::from_secs(180)).map(|(pk, _)| pk.clone()).collect())
            .unwrap_or_default();
        if self.calls.room.as_ref().is_some_and(|r| r.gid == gid) {
            v.push(self.me_pk());
        }
        v.sort();
        v.dedup();
        v
    }

    // ---------- the voice room ----------

    pub(super) fn join_room(&mut self, gid: &str) {
        let Some(g) = self.group(gid) else { return };
        if self.calls.room.as_ref().is_some_and(|r| r.gid == gid) {
            return;
        }
        if self.calls.room.is_some() {
            self.leave_room();
        }
        if self.calls.in_call() {
            self.hang_up();
        }
        self.stop_group_ring();
        self.apply_voice_settings();
        let was_empty = self.in_room(gid).iter().all(|pk| *pk == self.me_pk());
        self.calls.room = Some(Room {
            gid: gid.into(),
            joined: Instant::now(),
            peers: HashMap::new(),
            muted: false,
            deafened: false,
            muted_before: false,
            ping_at: Instant::now() + Duration::from_secs(60),
        });
        self.calls.voice.send(VCmd::Muted(false));
        self.calls.voice.send(VCmd::Deafened(false));
        self.send_group(&g, "vjoin", json!({}), None);
        if was_empty {
            self.send_group(&g, "vring", json!({}), None);
        }
        play(Sfx::Connect);
    }

    pub(super) fn leave_room(&mut self) {
        let Some(r) = self.calls.room.take() else { return };
        if let Some(g) = self.group(&r.gid) {
            self.send_group(&g, "vleave", json!({}), None);
        }
        self.calls.voice.send(VCmd::CloseAll);
        play(Sfx::End);
    }

    fn connect_room_peer(&mut self, pk: &str) {
        if let Some(r) = self.calls.room.as_mut() {
            r.peers.insert(pk.into(), RoomPeer::default());
        }
        self.calls.voice.send(VCmd::Offer(pk.into()));
    }

    fn close_room_peer(&mut self, pk: &str) {
        let Some(r) = self.calls.room.as_mut() else { return };
        if r.peers.remove(pk).is_some() {
            self.calls.voice.send(VCmd::Close(pk.into()));
        }
    }

    pub(super) fn stop_group_ring(&mut self) {
        if self.calls.ring.take().is_some() && self.calls.call.state == CallState::Idle {
            self.calls.ring_sound_at = None;
        }
    }

    // ---------- gamepad on the call card ----------

    /// The call card takes A / B / Y; true when it handled the press.
    pub(super) fn call_nav(&mut self, nav: Nav) -> bool {
        if let Some(r) = &self.calls.ring {
            let gid = r.gid.clone();
            match nav {
                Nav::Ok => self.join_room(&gid),
                Nav::Back => self.stop_group_ring(),
                _ => return true,
            }
            return true;
        }
        match self.calls.call.state {
            CallState::Incoming => match nav {
                Nav::Ok => self.accept_call(),
                Nav::Back => self.decline_call(),
                _ => {}
            },
            CallState::Outgoing | CallState::Connecting => match nav {
                Nav::Back => self.hang_up(),
                Nav::Options => self.toggle_mute(),
                _ => {}
            },
            _ => return false,
        }
        true
    }

    /// Settings-style menu for the call or voice room.
    pub(super) fn open_call_menu(&mut self, focus: usize) {
        let (vol, noise) = self.voice_settings();
        let (muted, deafened, title) = if let Some(r) = &self.calls.room {
            let t = self.group(&r.gid).map(|g| g.title(&self.me_pk())).unwrap_or_default();
            (r.muted, r.deafened, format!("Голосовой · {t}"))
        } else if self.calls.in_call() {
            let name = self.account().map(|a| self.display_name(&a, &self.calls.call.pk)).unwrap_or_default();
            (self.calls.call.muted, self.calls.call.deafened, format!("Звонок · {name}"))
        } else {
            return;
        };
        let mut items = vec![
            SetItem { key: SetKey::CallMute, label: if muted { "Включить микрофон" } else { "Выключить микрофон" }.into(), value: String::new(), danger: false },
            SetItem { key: SetKey::CallDeafen, label: if deafened { "Включить звук" } else { "Выключить звук" }.into(), value: "не слышать собеседников".into(), danger: false },
            SetItem { key: SetKey::CallVolume, label: "Громкость друзей".into(), value: format!("{vol:.0}%  ◀ ▶"), danger: false },
            SetItem { key: SetKey::CallMic, label: "Микрофон".into(), value: mic_label(&self.mic_setting()), danger: false },
            SetItem { key: SetKey::CallNoise, label: "Шумоподавление".into(), value: if noise { "Вкл" } else { "Выкл" }.into(), danger: false },
            SetItem { key: SetKey::CallVad, label: "Активация по голосу".into(), value: self.vad_label(), danger: false },
        ];
        if self.calls.call.state == CallState::Active {
            items.push(SetItem { key: SetKey::CallAdd, label: "Добавить друга в звонок".into(), value: "станет группой".into(), danger: false });
        }
        let hang = if self.calls.room.is_some() { "Выйти из голосового" } else { "Завершить звонок" };
        items.push(SetItem { key: SetKey::CallHangup, label: hang.into(), value: String::new(), danger: true });
        let focus = focus.min(items.len() - 1);
        self.modal = Some(Modal::Settings { menu: SetMenu::Call, title, items, focus, scroll: 0.0 });
    }

    pub(super) fn call_setting(&mut self, key: SetKey, focus: usize) {
        match key {
            SetKey::CallMute => self.toggle_mute(),
            SetKey::CallDeafen => self.toggle_deafen(),
            SetKey::CallVolume => {
                let (v, _) = self.voice_settings();
                self.set_volume(if v >= 200.0 { 0.0 } else { v + 25.0 });
            }
            SetKey::CallNoise => self.toggle_noise(),
            SetKey::CallMic => self.cycle_mic(),
            SetKey::CallVad => {
                let (on, db) = self.vad_setting();
                self.set_vad(!on, db);
            }
            SetKey::CallAdd => {
                let exclude = vec![self.calls.call.pk.clone()];
                return self.open_picker("Добавить в звонок", PickFor::AddToCall, &exclude, MAX_GROUP - 2);
            }
            SetKey::CallHangup => {
                self.modal = None;
                if self.calls.room.is_some() {
                    self.leave_room();
                } else {
                    self.hang_up();
                }
                return;
            }
            _ => {}
        }
        self.open_call_menu(focus);
    }

    /// ◀ ▶ on the volume row.
    pub(super) fn call_menu_side(&mut self, nav: Nav) -> bool {
        let Some(Modal::Settings { menu: SetMenu::Call, items, focus, .. }) = &self.modal else { return false };
        let key = items.get(*focus).map(|i| i.key);
        if !matches!(key, Some(SetKey::CallVolume | SetKey::CallVad)) || !matches!(nav, Nav::Left | Nav::Right) {
            return false;
        }
        let f = *focus;
        let step = if nav == Nav::Right { 1.0 } else { -1.0 };
        if key == Some(SetKey::CallVad) {
            // ◀ ▶ also switches it on: moving the threshold means you want it
            let (_, db) = self.vad_setting();
            self.set_vad(true, db + step * 5.0);
        } else {
            let (v, _) = self.voice_settings();
            self.set_volume(v + step * 10.0);
        }
        self.open_call_menu(f);
        true
    }

    // ---------- group menus and the friend picker ----------

    pub(super) fn open_group_menu(&mut self, gid: &str) {
        let Some(g) = self.group(gid) else { return };
        self.calls.menu_gid = Some(gid.into());
        let joined = self.calls.room.as_ref().is_some_and(|r| r.gid == gid);
        let mut items = vec![SetItem { key: SetKey::GroupVoice, label: if joined { "Выйти из голосового" } else { "Войти в голосовой" }.into(), value: String::new(), danger: false }];
        if g.members.len() < MAX_GROUP {
            items.push(SetItem { key: SetKey::GroupInvite, label: "Добавить друзей".into(), value: format!("{}/{MAX_GROUP}", g.members.len()), danger: false });
        }
        items.push(SetItem { key: SetKey::GroupRename, label: "Переименовать".into(), value: g.name.clone(), danger: false });
        items.push(SetItem { key: SetKey::GroupLeave, label: "Покинуть группу".into(), value: String::new(), danger: true });
        self.open(Modal::Settings { menu: SetMenu::Group, title: g.title(&self.me_pk()), items, focus: 0, scroll: 0.0 });
    }

    pub(super) fn group_setting(&mut self, key: SetKey) {
        let Some(gid) = self.calls.menu_gid.clone() else { return };
        match key {
            SetKey::GroupVoice => {
                self.modal = None;
                if self.calls.room.as_ref().is_some_and(|r| r.gid == gid) {
                    self.leave_room();
                } else {
                    self.join_room(&gid);
                }
            }
            SetKey::GroupInvite => {
                let Some(g) = self.group(&gid) else { return };
                let exclude: Vec<String> = g.members.iter().map(|m| m.pk.clone()).collect();
                self.open_picker("Добавить в группу", PickFor::Invite(gid), &exclude, MAX_GROUP - g.members.len());
            }
            SetKey::GroupRename => {
                let name = self.group(&gid).map(|g| g.name).unwrap_or_default();
                self.open(Modal::Keyboard(Box::new(crate::keyboard::Keyboard::new("Название группы", &name, crate::keyboard::Target::GroupRename, 32, false))));
            }
            SetKey::GroupLeave => {
                let title = self.group(&gid).map(|g| g.title(&self.me_pk())).unwrap_or_default();
                self.open(Modal::Confirm {
                    title: format!("Покинуть «{title}»?"),
                    text: "Вернуться получится, только если вас снова добавят.".into(),
                    yes: SetKey::GroupLeave,
                    yes_label: "Покинуть",
                    focus: 0,
                });
            }
            _ => {}
        }
    }

    pub(super) fn group_confirmed(&mut self) {
        if let Some(gid) = self.calls.menu_gid.take() {
            self.leave_group(&gid);
            self.toast("Вы покинули группу", false);
        }
    }

    pub(super) fn open_picker(&mut self, title: &str, purpose: PickFor, exclude: &[String], max: usize) {
        let Some(a) = self.account() else { return };
        let items: Vec<(String, String, bool)> = a.friends.iter().filter(|f| !exclude.contains(&f.pk)).map(|f| (f.pk.clone(), f.name.clone(), false)).collect();
        if items.is_empty() {
            return self.open(Modal::Message { title: title.into(), text: "Нет друзей, которых можно добавить".into() });
        }
        self.open(Modal::Pick { title: title.into(), items, focus: 0, scroll: 0.0, max, purpose });
    }

    pub(super) fn picked(&mut self, purpose: PickFor, pks: Vec<String>) {
        self.modal = None;
        if pks.is_empty() {
            return self.toast("Выберите хотя бы одного друга", true);
        }
        match purpose {
            PickFor::SendClip(path) => self.clip_send(&path, &pks),
            PickFor::NewGroup => {
                self.calls.pending_group = pks;
                self.open(Modal::Keyboard(Box::new(crate::keyboard::Keyboard::new("Название группы (можно пусто)", "", crate::keyboard::Target::GroupName, 32, false))));
            }
            PickFor::Invite(gid) => {
                self.invite(&gid, &pks);
                self.toast("Друзья добавлены в группу", false);
            }
            PickFor::AddToCall => {
                // the 1:1 call becomes a group voice room with everyone
                let other = self.calls.call.pk.clone();
                let mut all = vec![other.clone()];
                all.extend(pks);
                let Some(gid) = self.create_group("", &all) else { return };
                let id = self.calls.call.id.clone();
                self.send_call(&other, "move", json!({ "id": id, "gid": gid }));
                self.end_call(None);
                self.join_room(&gid);
                self.toast("Звонок перенесён в группу — друзьям пришло приглашение", false);
            }
        }
    }

    pub(super) fn group_text(&mut self, target: crate::keyboard::Target, text: String) {
        match target {
            crate::keyboard::Target::GroupName => {
                let pks = std::mem::take(&mut self.calls.pending_group);
                if let Some(gid) = self.create_group(&text, &pks) {
                    let title = self.group(&gid).map(|g| g.title(&self.me_pk())).unwrap_or_default();
                    self.toast(&format!("Группа «{title}» создана"), false);
                    self.social.open_chat(&format!("g:{gid}"));
                }
            }
            crate::keyboard::Target::GroupRename => {
                if let Some(gid) = self.calls.menu_gid.clone() {
                    self.update_group(&gid, |g| g.name = text.trim().into());
                    self.invite(&gid, &[]);
                }
            }
            _ => {}
        }
    }

    // ---------- painting ----------

    /// The full-screen call card: incoming / outgoing / connecting, or a group calling you in.
    pub(super) fn paint_call_card(&self, ui: &egui::Ui, rect: Rect, s: f32) {
        let (key, name, status, presence, incoming) = if let Some(r) = &self.calls.ring {
            let Some(g) = self.group(&r.gid) else { return };
            let others: Vec<String> = self.in_room(&g.id).into_iter().filter(|pk| *pk != self.me_pk()).map(|pk| self.member_name(&g, &pk)).collect();
            (g.id.clone(), g.title(&self.me_pk()), format!("{} зовёт в голосовой", self.member_name(&g, &r.from)), format!("В голосовом: {}", if others.is_empty() { "—".into() } else { others.join(", ") }), true)
        } else {
            let c = &self.calls.call;
            let status = match c.state {
                CallState::Incoming => "Входящий звонок",
                CallState::Outgoing => "Звоним",
                CallState::Connecting => "Соединяем",
                _ => return,
            };
            let Some(a) = self.account() else { return };
            let (on, text, _) = self.presence_of(&c.pk);
            let presence = if on { text } else { "Не в сети — звонок дойдёт, когда друг откроет OTIUM".into() };
            (c.pk.clone(), self.display_name(&a, &c.pk), status.to_string(), presence, c.state == CallState::Incoming)
        };
        let p = ui.painter();
        p.rect_filled(rect, 0.0, Color32::from_rgba_unmultiplied(6, 8, 14, 240));
        let c = pos2(rect.center().x, rect.height() * 0.36);
        let t = ui.input(|i| i.time) as f32;
        let col = draw::name_color(&key);
        // pulsing rings
        for k in 0..3 {
            let phase = ((t * 0.7 + k as f32 / 3.0) % 1.0) as f32;
            p.circle_stroke(c, 90.0 * s + phase * 120.0 * s, Stroke::new(3.0 * s, col.gamma_multiply(0.5 * (1.0 - phase))));
        }
        p.circle_filled(c, 90.0 * s, col);
        let letter = name.chars().next().map(|ch| ch.to_uppercase().to_string()).unwrap_or_default();
        p.text(c, Align2::CENTER_CENTER, letter, bold(80.0 * s), Color32::WHITE);
        let kicker = if self.calls.ring.is_some() { "Групповой звонок · OTIUM" } else { "Голосовой звонок · OTIUM" };
        p.text(pos2(rect.center().x, 110.0 * s), Align2::CENTER_CENTER, kicker, semibold(20.0 * s), white(0.6));
        p.text(pos2(rect.center().x, c.y + 150.0 * s), Align2::CENTER_CENTER, &name, bold(48.0 * s), Color32::WHITE);
        let wait = self.calls.call.ring_started.map(|r| mmss(r.elapsed())).unwrap_or_default();
        let st = if self.calls.ring.is_some() { status.clone() } else { format!("{status}…  {wait}") };
        p.text(pos2(rect.center().x, c.y + 206.0 * s), Align2::CENTER_CENTER, st, font(24.0 * s), white(0.75));
        p.text(pos2(rect.center().x, c.y + 244.0 * s), Align2::CENTER_CENTER, presence, font(19.0 * s), white(0.5));

        let glyph = |b: usize| -> &str { if self.pad_kind() == crate::input::PadKind::PlayStation { ["✕", "○", "△"][b] } else { ["A", "B", "Y"][b] } };
        let buttons: Vec<(&str, Color32, usize)> = if incoming {
            vec![(if self.calls.ring.is_some() { "Войти" } else { "Принять" }, GREEN, 0), (if self.calls.ring.is_some() { "Не сейчас" } else { "Отклонить" }, RED, 1)]
        } else {
            vec![(if self.calls.call.muted { "Микрофон выкл." } else { "Микрофон" }, white(0.15), 2), ("Отменить", RED, 1)]
        };
        let y = c.y + 340.0 * s;
        let total = buttons.len() as f32 * 260.0 * s + (buttons.len() as f32 - 1.0) * 30.0 * s;
        let mut x = rect.center().x - total / 2.0;
        for (label, color, b) in buttons {
            let r = Rect::from_min_size(pos2(x, y), vec2(260.0 * s, 72.0 * s));
            p.rect_filled(r, CornerRadius::same((36.0 * s) as u8), color);
            p.text(r.center() - vec2(14.0 * s, 0.0), Align2::CENTER_CENTER, label, semibold(23.0 * s), Color32::WHITE);
            draw::pad_glyph(p, pos2(r.right() - 34.0 * s, r.center().y), 14.0 * s, glyph(b), Color32::WHITE);
            x += 290.0 * s;
        }
    }

    /// What the overlay over a game shows about calls: the incoming card, or the voice widget.
    fn overlay_call(&self) -> Option<crate::overlay::CallView> {
        use crate::overlay::{CallView, Person};
        let me = self.me_pk();
        if let Some(r) = &self.calls.ring {
            let g = self.group(&r.gid)?;
            return Some(CallView::Incoming { key: g.id.clone(), name: g.title(&me), kicker: format!("{} зовёт в голосовой", self.member_name(&g, &r.from)), group: true });
        }
        let speaking = |k: &str| self.calls.speaking.get(k).copied().unwrap_or(false);
        if let Some(r) = &self.calls.room {
            let g = self.group(&r.gid)?;
            let mut people = vec![Person { key: me.clone(), name: self.my_name(), muted: r.muted, deafened: r.deafened, speaking: speaking(""), connected: true }];
            for (pk, p) in &r.peers {
                people.push(Person { key: pk.clone(), name: self.member_name(&g, pk), muted: p.muted, deafened: p.deafened, speaking: speaking(pk), connected: p.connected });
            }
            return Some(CallView::Active { title: g.title(&me), status: None, started: Some(r.joined), people });
        }
        let c = &self.calls.call;
        let a = self.account()?;
        let name = self.display_name(&a, &c.pk);
        let status = match c.state {
            CallState::Idle => return None,
            CallState::Incoming => return Some(CallView::Incoming { key: c.pk.clone(), name, kicker: "Входящий звонок".into(), group: false }),
            CallState::Outgoing => Some("Звоним…".to_string()),
            CallState::Connecting => Some("Соединяем…".to_string()),
            CallState::Active => None,
        };
        let people = vec![
            Person { key: me.clone(), name: self.my_name(), muted: c.muted, deafened: c.deafened, speaking: speaking(""), connected: true },
            Person { key: c.pk.clone(), name, muted: c.remote_muted, deafened: c.remote_deafened, speaking: speaking(&c.pk), connected: c.state == CallState::Active },
        ];
        Some(CallView::Active { title: "Голосовой звонок".into(), status, started: c.started, people })
    }

    /// `OTIUM_OVERLAY_DEMO=call|incoming`: the overlay shows a made-up call (whether OTIUM is in
    /// front or not) and a notification every few seconds, to test it without a friend.
    fn overlay_demo(&mut self, kind: &str) -> crate::overlay::CallView {
        use crate::overlay::{CallView, Person};
        let t = self.started.elapsed().as_secs_f32();
        if self.overlay_demo_toasts < (t / 4.0) as u32 + 1 {
            self.overlay_demo_toasts += 1;
            let n = self.overlay_demo_toasts;
            if n % 2 == 1 {
                self.overlay.toast("Аня", &format!("Сообщение номер {n}: заходи в голосовой, мы уже собрались, длинный текст переносится на несколько строк"), Some(("demo-anya".into(), "Аня".into())), false);
            } else {
                self.overlay.toast("OTIUM", "Не удалось соединиться — проверьте интернет", None, true);
            }
        }
        if kind == "incoming" {
            return CallView::Incoming { key: "demo-anya".into(), name: "Аня".into(), kicker: "Входящий звонок".into(), group: false };
        }
        let talk = (t * 1.3) as u32 % 3;
        let people = vec![
            Person { key: "demo-me".into(), name: self.my_name(), muted: false, deafened: false, speaking: talk == 0, connected: true },
            Person { key: "demo-anya".into(), name: "Аня".into(), muted: false, deafened: false, speaking: talk == 1, connected: true },
            Person { key: "demo-max".into(), name: "Максим".into(), muted: true, deafened: false, speaking: false, connected: true },
            Person { key: "demo-fox".into(), name: "Лиса".into(), muted: true, deafened: true, speaking: false, connected: true },
            Person { key: "demo-kot".into(), name: "Кот".into(), muted: false, deafened: false, speaking: false, connected: false },
        ];
        CallView::Active { title: "Голосовой звонок".into(), status: None, started: Some(self.started), people }
    }

    /// The overlay window over the game; only while OTIUM itself isn't in front.
    pub(super) fn paint_overlay(&mut self, ctx: &egui::Context) {
        use crate::overlay::Action;
        // not while OTIUM is starting: its own window is about to take the focus, and a window
        // shown before that could get it instead
        if self.splash_start.is_some() || self.started.elapsed() < Duration::from_secs(2) {
            return;
        }
        let mut call = if self.away() { self.overlay_call() } else { None };
        if let Some(demo) = std::env::var("OTIUM_OVERLAY_DEMO").ok() {
            call = Some(self.overlay_demo(&demo));
        }
        let own = self.own.load(std::sync::atomic::Ordering::Relaxed);
        let action = self.overlay.show(ctx, own, call);
        if std::env::var_os("OTIUM_OVERLAY_DEMO").is_some() && action.is_some() {
            eprintln!("overlay {action:?}");
        }
        match action {
            Some(Action::Accept) => {
                if let Some(r) = &self.calls.ring {
                    let gid = r.gid.clone();
                    self.join_room(&gid);
                } else {
                    self.accept_call();
                }
            }
            Some(Action::Decline) => {
                if self.calls.ring.is_some() {
                    self.stop_group_ring();
                } else {
                    self.decline_call();
                }
            }
            Some(Action::OpenCall) => {
                self.show_window();
                self.refocus = true;
                self.open_call_menu(0);
            }
            None => {}
        }
    }

    /// The compact pill at the top while in a call or a voice room.
    pub(super) fn paint_call_pill(&self, ui: &egui::Ui, rect: Rect, s: f32) {
        let (title, started, muted, deafened, others) = if let Some(r) = &self.calls.room {
            let Some(g) = self.group(&r.gid) else { return };
            (format!("🔊 {}", g.title(&self.me_pk())), r.joined, r.muted, r.deafened, format!("{} в голосовом", r.peers.values().filter(|p| p.connected).count() + 1))
        } else if self.calls.call.state == CallState::Active {
            let Some(a) = self.account() else { return };
            let c = &self.calls.call;
            let remote = if c.remote_deafened { " · без звука" } else if c.remote_muted { " · микрофон выкл." } else { "" };
            (self.display_name(&a, &c.pk), c.started.unwrap_or_else(Instant::now), c.muted, c.deafened, format!("{}{remote}", mmss(c.started.map(|t| t.elapsed()).unwrap_or_default())))
        } else {
            return;
        };
        let p = ui.painter();
        let r = Rect::from_center_size(pos2(rect.center().x, 62.0 * s), vec2(460.0 * s, 64.0 * s));
        p.rect_filled(r, CornerRadius::same((32.0 * s) as u8), Color32::from_rgb(22, 60, 40));
        p.rect_stroke(r, CornerRadius::same((32.0 * s) as u8), Stroke::new(1.5_f32, GREEN.gamma_multiply(0.6)), egui::StrokeKind::Inside);
        let speaking_any = self.calls.speaking.iter().any(|(k, v)| !k.is_empty() && *v);
        p.circle_filled(pos2(r.left() + 34.0 * s, r.center().y), 9.0 * s, if speaking_any { GREEN } else { GREEN.gamma_multiply(0.4) });
        p.text(pos2(r.left() + 58.0 * s, r.center().y - 11.0 * s), Align2::LEFT_CENTER, title, semibold(19.0 * s), Color32::WHITE);
        let sub = if self.calls.room.is_some() { format!("{} · {}", mmss(started.elapsed()), others) } else { others };
        p.text(pos2(r.left() + 58.0 * s, r.center().y + 13.0 * s), Align2::LEFT_CENTER, sub, font(15.0 * s), white(0.7));
        let icons = format!("{}{}", if muted { "🎙̸ " } else { "" }, if deafened { "🔇" } else { "" });
        p.text(pos2(r.right() - 24.0 * s, r.center().y), Align2::RIGHT_CENTER, if icons.is_empty() { "Home — меню".to_string() } else { icons }, font(15.0 * s), white(0.7));
    }

    /// Right side of the friends panel for a group: header, voice stage, messages.
    pub(super) fn paint_group_chat(&mut self, ui: &egui::Ui, right: Rect, s: f32, gid: &str) {
        let Some(g) = self.group(gid) else { return };
        let me = self.me_pk();
        let p = ui.painter();
        let head = Rect::from_min_size(right.min, vec2(right.width(), 84.0 * s));
        let av = pos2(head.left() + 48.0 * s, head.center().y);
        p.circle_filled(av, 26.0 * s, draw::name_color(&g.id));
        let title = g.title(&me);
        p.text(av, Align2::CENTER_CENTER, title.chars().next().map(|c| c.to_uppercase().to_string()).unwrap_or_default(), semibold(22.0 * s), Color32::WHITE);
        p.text(pos2(head.left() + 90.0 * s, head.center().y - 12.0 * s), Align2::LEFT_CENTER, &title, semibold(23.0 * s), Color32::WHITE);
        let members = g.members.iter().map(|m| if m.pk == me { "вы".to_string() } else { m.name.clone() }).collect::<Vec<_>>().join(" · ");
        p.text(pos2(head.left() + 90.0 * s, head.center().y + 14.0 * s), Align2::LEFT_CENTER, members, font(16.0 * s), white(0.5));
        p.line_segment([pos2(right.left(), head.bottom()), pos2(right.right(), head.bottom())], Stroke::new(1.0_f32, white(0.06)));

        let mut top = head.bottom();
        let joined = self.calls.room.as_ref().filter(|r| r.gid == gid);
        if let Some(r) = joined {
            // voice stage: a tile per participant, green ring while talking
            let stage = Rect::from_min_size(pos2(right.left() + 20.0 * s, top + 16.0 * s), vec2(right.width() - 40.0 * s, 170.0 * s));
            p.rect_filled(stage, CornerRadius::same((16.0 * s) as u8), Color32::from_rgb(14, 30, 24));
            p.text(stage.left_top() + vec2(18.0 * s, 14.0 * s), Align2::LEFT_TOP, format!("● Голосовой · {}", mmss(r.joined.elapsed())), semibold(16.0 * s), GREEN);
            let mut people: Vec<(String, String, bool, bool, bool)> = vec![(me.clone(), self.my_name(), r.muted, r.deafened, true)];
            for (pk, pp) in &r.peers {
                people.push((pk.clone(), self.member_name(&g, pk), pp.muted, pp.deafened, pp.connected));
            }
            for (i, (pk, name, muted, deaf, connected)) in people.iter().enumerate() {
                let c = pos2(stage.left() + 80.0 * s + i as f32 * 130.0 * s, stage.top() + 88.0 * s);
                let talking = self.calls.speaking.get(if *pk == me { "" } else { pk.as_str() }).copied().unwrap_or(false);
                if talking {
                    p.circle_stroke(c, 38.0 * s, Stroke::new(4.0 * s, GREEN));
                }
                p.circle_filled(c, 32.0 * s, draw::name_color(pk).gamma_multiply(if *connected { 1.0 } else { 0.4 }));
                p.text(c, Align2::CENTER_CENTER, name.chars().next().map(|ch| ch.to_uppercase().to_string()).unwrap_or_default(), semibold(26.0 * s), Color32::WHITE);
                let label = if !connected { "соединяем…".to_string() } else if *deaf { format!("{name} 🔇") } else if *muted { format!("{name} 🎙̸") } else { name.clone() };
                p.text(c + vec2(0.0, 54.0 * s), Align2::CENTER_CENTER, label, font(15.0 * s), white(0.85));
            }
            top = stage.bottom();
        } else {
            let live: Vec<String> = self.in_room(gid).into_iter().filter(|pk| *pk != me).map(|pk| self.member_name(&g, &pk)).collect();
            if !live.is_empty() {
                p.text(pos2(right.left() + 24.0 * s, top + 22.0 * s), Align2::LEFT_CENTER, format!("🔊 В голосовом: {}", live.join(", ")), semibold(17.0 * s), GREEN);
                top += 40.0 * s;
            }
        }

        let focus = self.social.chat_focused() && self.modal.is_none();
        let input = Rect::from_min_max(pos2(right.left() + 20.0 * s, right.bottom() - 84.0 * s), pos2(right.right() - 20.0 * s, right.bottom() - 20.0 * s));
        if focus {
            draw::glow(p, input, 16.0 * s, Color32::WHITE, 12.0 * s, 0.4);
        }
        p.rect_filled(input, CornerRadius::same((16.0 * s) as u8), if focus { white(0.14) } else { white(0.06) });
        let hint = if joined.is_some() { "Сообщение в группу · Y — меню группы" } else { "Сообщение в группу · Y — голосовой и меню" };
        p.text(pos2(input.left() + 22.0 * s, input.center().y), Align2::LEFT_CENTER, hint, font(19.0 * s), white(0.45));

        let area = Rect::from_min_max(pos2(right.left() + 24.0 * s, top + 12.0 * s), pos2(right.right() - 24.0 * s, input.top() - 14.0 * s));
        if g.history.is_empty() {
            p.text(area.center(), Align2::CENTER_CENTER, "Это начало группы. Позовите всех в голосовой 🎧", font(20.0 * s), white(0.5));
            return;
        }
        let names: HashMap<String, String> = g.members.iter().map(|m| (m.pk.clone(), self.member_name(&g, &m.pk))).collect();
        let mut scroll = self.social.chat_scroll();
        super::social::paint_messages(p, area, s, &g.history, &me, &mut scroll, &|pk: &str| names.get(pk).cloned().unwrap_or_else(|| "Игрок".into()));
        self.social.set_chat_scroll(scroll);
    }
}

const VAD_DEFAULT_DB: f32 = -45.0;
const VAD_MIN_DB: f32 = -80.0;
const VAD_MAX_DB: f32 = -10.0;

fn fmt_db(db: f32) -> String {
    format!("{} дБ", (db.round() as i32).to_string().replace('-', "−"))
}

/// Shown name of the chosen microphone: Windows names look like "Микрофон (Realtek Audio)".
fn mic_label(id: &str) -> String {
    if id.is_empty() || !crate::voice::mics().iter().any(|m| m == id) {
        "По умолчанию".into()
    } else {
        id.to_string()
    }
}

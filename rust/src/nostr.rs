//! Accounts, friends and chat over Nostr — the same protocol the Electron build speaks, so both
//! versions talk to each other: NIP-04 encrypted DMs (kind 4), profiles tagged `ps5-launcher`
//! (kind 0), the friends list as a contact list (kind 3) and the "playing …" status (kind 30315).
//! Runs on its own thread with a tokio runtime; the UI talks to it through channels.

use nostr_sdk::prelude::*;
use std::time::Duration;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};

pub const RELAYS: [&str; 3] = ["wss://relay.damus.io", "wss://nos.lol", "wss://relay.primal.net"];
pub const APP_TAG: &str = "ps5-launcher";

#[derive(Debug)]
pub enum Cmd {
    /// connect with this secret key (hex)
    Start(String),
    Stop,
    SendDm { to: String, text: String },
    /// sign and publish an event: (kind, content, tags)
    Publish(u16, String, Vec<Vec<String>>),
    /// follow profiles and statuses of these public keys
    Watch(Vec<String>),
    /// players of the app, for "find friends"
    Search,
    /// the profile name and contact list of an account, for signing in with a key
    FetchAccount(String),
    /// a friend's published library (kind 30078, d = "ps5l-library")
    FetchLibrary(String),
}

#[derive(Debug, Clone)]
pub enum Msg {
    Dm { id: String, other: String, mine: bool, text: String, at: f64 },
    Profile { pk: String, name: String, at: u64 },
    Presence { pk: String, text: String, at: f64 },
    Players(Vec<(String, String)>),
    Account { name: Option<String>, friends: Vec<(String, String)> },
    SendFailed,
    /// the library a player published (None = none found)
    Library { pk: String, content: Option<String> },
}

pub struct Nostr {
    tx: UnboundedSender<Cmd>,
}

impl Nostr {
    pub fn start(send: impl Fn(Msg) + Send + 'static) -> Self {
        let (tx, rx) = unbounded_channel();
        std::thread::Builder::new()
            .name("nostr".into())
            .spawn(move || {
                let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().expect("tokio");
                rt.block_on(run(rx, send));
            })
            .ok();
        Self { tx }
    }

    pub fn send(&self, c: Cmd) {
        let _ = self.tx.send(c);
    }
}

/// Public key (hex) for a secret key (hex or nsec).
pub fn public_key(sk: &str) -> Option<String> {
    Keys::parse(sk.trim()).ok().map(|k| k.public_key().to_hex())
}

pub fn generate_secret() -> String {
    Keys::generate().secret_key().to_secret_hex()
}

/// The secret key as hex, from hex or `nsec1…`.
pub fn secret_hex(sk: &str) -> Option<String> {
    Keys::parse(sk.trim()).ok().map(|k| k.secret_key().to_secret_hex())
}

pub fn npub(pk: &str) -> String {
    PublicKey::from_hex(pk).ok().and_then(|p| p.to_bech32().ok()).unwrap_or_default()
}

pub fn nsec(sk: &str) -> String {
    Keys::parse(sk).ok().and_then(|k| k.secret_key().to_bech32().ok()).unwrap_or_default()
}

/// The 6-letter friend code the Electron build shows: the end of the npub, upper case.
pub fn short_code(pk: &str) -> String {
    let n = npub(pk);
    n.chars().skip(n.chars().count().saturating_sub(6)).collect::<String>().to_uppercase()
}

/// hex public key from hex or `npub1…`
pub fn parse_public(s: &str) -> Option<String> {
    PublicKey::parse(s.trim()).ok().map(|p| p.to_hex())
}

struct Session {
    client: Client,
    keys: Keys,
    friends_sub: Option<SubscriptionId>,
}

async fn run(mut rx: UnboundedReceiver<Cmd>, send: impl Fn(Msg) + Send + 'static) {
    let mut session: Option<Session> = None;
    let mut notifications: Option<std::pin::Pin<Box<dyn futures::Stream<Item = ClientNotification> + Send>>> = None;
    loop {
        tokio::select! {
            cmd = rx.recv() => {
                let Some(cmd) = cmd else { return };
                match cmd {
                    Cmd::Start(sk) => {
                        if let Some(s) = session.take() {
                            s.client.shutdown().await;
                        }
                        let Ok(keys) = Keys::parse(&sk) else { continue };
                        let client = Client::new();
                        for r in RELAYS {
                            let _ = client.add_relay(r).await;
                        }
                        client.connect().await;
                        notifications = Some(client.notifications());
                        let me = keys.public_key();
                        let since = Timestamp::from_secs(Timestamp::now().as_secs().saturating_sub(30 * 86400));
                        let _ = client.subscribe(Filter::new().kind(Kind::from(4u16)).pubkey(me).since(since)).await;
                        let _ = client.subscribe(Filter::new().kind(Kind::from(4u16)).author(me).since(since)).await;
                        session = Some(Session { client, keys, friends_sub: None });
                    }
                    Cmd::Stop => {
                        if let Some(s) = session.take() {
                            s.client.shutdown().await;
                        }
                        notifications = None;
                    }
                    Cmd::SendDm { to, text } => {
                        let Some(s) = &session else { continue };
                        let Ok(pk) = PublicKey::from_hex(&to) else { continue };
                        let Ok(content) = nip04::encrypt(s.keys.secret_key(), &pk, &text) else { continue };
                        let Ok(ev) = EventBuilder::new(Kind::from(4u16), content).tag(Tag::public_key(pk)).finalize(&s.keys) else { continue };
                        // shown right away, before the relays confirm
                        send(Msg::Dm { id: ev.id.to_hex(), other: to.clone(), mine: true, text, at: ev.created_at.as_secs() as f64 * 1000.0 });
                        let ok = matches!(s.client.send_event(&ev).await, Ok(o) if !o.success.is_empty());
                        if !ok {
                            send(Msg::SendFailed);
                        }
                    }
                    Cmd::Publish(kind, content, tags) => {
                        let Some(s) = &session else { continue };
                        let tags: Vec<Tag> = tags.into_iter().filter_map(|t| Tag::parse(t).ok()).collect();
                        if let Ok(ev) = EventBuilder::new(Kind::from(kind), content).tags(tags).finalize(&s.keys) {
                            let _ = s.client.send_event(&ev).await;
                        }
                    }
                    Cmd::Watch(pks) => {
                        let Some(s) = &mut session else { continue };
                        if let Some(id) = s.friends_sub.take() {
                            let _ = s.client.unsubscribe(&id).await;
                        }
                        let authors: Vec<PublicKey> = pks.iter().filter_map(|p| PublicKey::from_hex(p).ok()).collect();
                        if authors.is_empty() {
                            continue;
                        }
                        let f = Filter::new().kinds([Kind::from(0u16), Kind::from(30315u16)]).authors(authors);
                        if let Ok(out) = s.client.subscribe(f).await {
                            s.friends_sub = Some(out.value);
                        }
                    }
                    Cmd::Search => {
                        let Some(s) = &session else { continue };
                        let f = Filter::new().kind(Kind::from(0u16)).hashtag(APP_TAG).limit(500);
                        let evs = s.client.fetch_events(f).timeout(Duration::from_secs(8)).await.unwrap_or_default();
                        let mut latest: std::collections::HashMap<String, (u64, String)> = Default::default();
                        for e in evs {
                            let name = profile_name(&e.content).unwrap_or_default();
                            let pk = e.pubkey.to_hex();
                            let at = e.created_at.as_secs();
                            if latest.get(&pk).is_none_or(|(t, _)| *t < at) {
                                latest.insert(pk, (at, name));
                            }
                        }
                        send(Msg::Players(latest.into_iter().map(|(pk, (_, n))| (pk, n)).collect()));
                    }
                    Cmd::FetchLibrary(pk) => {
                        let (Ok(author), Some(s)) = (PublicKey::from_hex(&pk), &session) else {
                            send(Msg::Library { pk, content: None });
                            continue;
                        };
                        let f = Filter::new().kind(Kind::from(30078u16)).author(author).identifier("ps5l-library").limit(3);
                        let evs = s.client.fetch_events(f).timeout(Duration::from_secs(8)).await.unwrap_or_default();
                        let content = evs.iter().max_by_key(|e| e.created_at).map(|e| e.content.clone());
                        send(Msg::Library { pk, content });
                    }
                    Cmd::FetchAccount(pk) => {
                        let Ok(pk) = PublicKey::from_hex(&pk) else { continue };
                        let client = match &session { Some(s) => s.client.clone(), None => continue };
                        let prof = client.fetch_events(Filter::new().kind(Kind::from(0u16)).author(pk).limit(1)).timeout(Duration::from_secs(8)).await.unwrap_or_default();
                        let contacts = client.fetch_events(Filter::new().kind(Kind::from(3u16)).author(pk).limit(1)).timeout(Duration::from_secs(8)).await.unwrap_or_default();
                        let name = prof.iter().max_by_key(|e| e.created_at).and_then(|e| profile_name(&e.content));
                        let friends = contacts
                            .iter()
                            .max_by_key(|e| e.created_at)
                            .map(|e| {
                                e.tags.iter().filter(|t| t.kind() == "p").filter_map(|t| {
                                    let v = t.as_slice();
                                    let pk = v.get(1)?.clone();
                                    let name = v.get(3).cloned().filter(|n| !n.is_empty()).unwrap_or_else(|| format!("Игрок {}", &pk[..6.min(pk.len())]));
                                    Some((pk, name))
                                }).collect()
                            })
                            .unwrap_or_default();
                        send(Msg::Account { name, friends });
                    }
                }
            }
            n = async { match &mut notifications { Some(s) => s.next().await, None => std::future::pending().await } } => {
                let Some(ClientNotification::Event { event, .. }) = n else { continue };
                let Some(s) = &session else { continue };
                handle_event(&s.keys, &event, &send);
            }
        }
    }
}

fn profile_name(content: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(content).ok()?;
    v["name"].as_str().or(v["display_name"].as_str()).map(str::to_string).filter(|n| !n.is_empty())
}

fn handle_event(keys: &Keys, e: &Event, send: &impl Fn(Msg)) {
    let me = keys.public_key();
    match e.kind.as_u16() {
        4 => {
            let mine = e.pubkey == me;
            let other = if mine { e.tags.public_keys().next() } else { Some(e.pubkey) };
            let Some(other) = other.filter(|o| *o != me) else { return };
            let Ok(text) = nip04::decrypt(keys.secret_key(), &other, &e.content) else { return };
            send(Msg::Dm { id: e.id.to_hex(), other: other.to_hex(), mine, text, at: e.created_at.as_secs() as f64 * 1000.0 });
        }
        0 => {
            if let Some(name) = profile_name(&e.content) {
                send(Msg::Profile { pk: e.pubkey.to_hex(), name, at: e.created_at.as_secs() });
            }
        }
        30315 => send(Msg::Presence { pk: e.pubkey.to_hex(), text: e.content.clone(), at: e.created_at.as_secs() as f64 * 1000.0 }),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_and_codes() {
        let sk = generate_secret();
        let pk = public_key(&sk).unwrap();
        assert_eq!(pk.len(), 64);
        assert!(npub(&pk).starts_with("npub1"));
        assert_eq!(short_code(&pk).len(), 6);
        assert_eq!(secret_hex(&nsec(&sk)).unwrap(), sk);
        assert_eq!(parse_public(&npub(&pk)).unwrap(), pk);
    }

    #[test]
    fn nip04_round_trip() {
        let a = Keys::generate();
        let b = Keys::generate();
        let c = nip04::encrypt(a.secret_key(), &b.public_key(), "привет").unwrap();
        assert_eq!(nip04::decrypt(b.secret_key(), &a.public_key(), &c).unwrap(), "привет");
    }
}

#[cfg(test)]
mod live {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// Connects with the real account from library.json and reads DMs (only counts are printed).
    #[test]
    #[ignore]
    fn live_reads_history() {
        let lib = crate::data::Library::load();
        let social = &lib.extra["social"];
        let sk = social["sk"].as_str().expect("no account").to_string();
        let known: std::collections::HashSet<String> = social["history"]
            .as_object()
            .map(|h| h.values().flat_map(|l| l.as_array().cloned().unwrap_or_default()).filter_map(|m| m["id"].as_str().map(str::to_string)).collect())
            .unwrap_or_default();
        let got: Arc<Mutex<Vec<Msg>>> = Default::default();
        let g = got.clone();
        let n = Nostr::start(move |m| g.lock().unwrap().push(m));
        n.send(Cmd::Start(sk));
        std::thread::sleep(Duration::from_secs(12));
        let got = got.lock().unwrap();
        let dms: Vec<&String> = got.iter().filter_map(|m| if let Msg::Dm { id, .. } = m { Some(id) } else { None }).collect();
        let matched = dms.iter().filter(|id| known.contains(id.as_str())).count();
        println!("dms received: {}, already in history: {}, history size: {}", dms.len(), matched, known.len());
        assert!(!dms.is_empty(), "no DMs arrived");
    }
}

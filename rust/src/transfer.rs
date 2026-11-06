//! Clips straight from PC to PC over a WebRTC data channel, encrypted the way calls are and with no
//! server in between: the two sides agree through the chat (offer and answer sent whole, like a call).
//! The one who wants the file makes the offer. Both have to be online at the same moment.

use bytes::BytesMut;
use rtc::interceptor::Registry;
use rtc::peer_connection::configuration::interceptor_registry::register_default_interceptors;
use rtc::peer_connection::configuration::media_engine::MediaEngine;
use rtc::peer_connection::configuration::RTCConfigurationBuilder;
use rtc::peer_connection::sdp::RTCSessionDescription;
use rtc::peer_connection::transport::RTCIceServer;
use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use webrtc::data_channel::{DataChannel, DataChannelEvent};
use webrtc::peer_connection::{PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler, RTCIceGatheringState, RTCPeerConnectionState};

const ICE: [&str; 2] = ["stun:stun.l.google.com:19302", "stun:stun.cloudflare.com:3478"];
const CHUNK: usize = 60 * 1024;
/// nothing moved for this long: the other side is gone (or never answered)
const IDLE: Duration = Duration::from_secs(40);
/// what a friend may send at most (a 30-second clip is some 50 MB)
pub const MAX_SIZE: u64 = 500 * 1024 * 1024;

#[derive(Debug)]
pub enum Cmd {
    /// get this file: `path` is where it goes, `size` what was promised
    Fetch { id: String, path: PathBuf, size: u64 },
    /// the answer to our offer
    Accepted { id: String, sdp: String },
    /// someone asks for `path`
    Serve { id: String, path: PathBuf, offer: String },
}

#[derive(Debug, Clone)]
pub enum Event {
    /// our offer (or answer), ready to go over the chat
    Sdp { id: String, sdp: String, offer: bool },
    Received { id: String, path: PathBuf },
    Sent { id: String },
    /// `begun`: it broke halfway, not just "nobody answered"
    Failed { id: String, begun: bool },
}

pub struct Transfer {
    tx: UnboundedSender<Cmd>,
}

type Send_ = Arc<dyn Fn(Event) + Send + Sync>;

impl Transfer {
    pub fn start(send: impl Fn(Event) + Send + Sync + 'static) -> Self {
        let (tx, rx) = unbounded_channel();
        std::thread::Builder::new()
            .name("transfer".into())
            .spawn(move || {
                let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().expect("tokio");
                rt.block_on(run(rx, Arc::new(send)));
            })
            .ok();
        Self { tx }
    }

    pub fn send(&self, c: Cmd) {
        let _ = self.tx.send(c);
    }
}

/// One file on its way, in either direction.
struct Job {
    id: String,
    send: Send_,
    pc: Mutex<Option<Arc<dyn PeerConnection>>>,
    done: AtomicBool,
    begun: AtomicBool,
    /// milliseconds since `at` of the last sign of life
    last: AtomicU64,
    at: Instant,
    gathered: tokio::sync::watch::Sender<bool>,
    /// serving: the file
    serve: Option<PathBuf>,
}

impl Job {
    fn alive(&self) {
        self.last.store(self.at.elapsed().as_millis() as u64, Ordering::Relaxed);
    }

    fn close(&self) {
        let pc = self.pc.lock().unwrap().take();
        if let Some(pc) = pc {
            tokio::spawn(async move {
                let _ = pc.close().await;
            });
        }
    }

    /// The end, once: what happened is told and the connection goes.
    fn finish(&self, e: Event) {
        if !self.done.swap(true, Ordering::Relaxed) {
            (self.send)(e);
            self.close();
        }
    }

    fn fail(&self) {
        self.finish(Event::Failed { id: self.id.clone(), begun: self.begun.load(Ordering::Relaxed) });
    }
}

struct Handler {
    job: Arc<Job>,
}

#[async_trait::async_trait]
impl PeerConnectionEventHandler for Handler {
    async fn on_ice_gathering_state_change(&self, state: RTCIceGatheringState) {
        if state == RTCIceGatheringState::Complete {
            let _ = self.job.gathered.send(true);
        }
    }

    async fn on_connection_state_change(&self, state: RTCPeerConnectionState) {
        if matches!(state, RTCPeerConnectionState::Failed) {
            self.job.fail();
        }
    }

    async fn on_data_channel(&self, dc: Arc<dyn DataChannel>) {
        if let Some(path) = self.job.serve.clone() {
            tokio::spawn(serve(dc, path, self.job.clone()));
        }
    }
}

async fn run(mut rx: UnboundedReceiver<Cmd>, send: Send_) {
    let mut jobs: HashMap<String, Arc<Job>> = HashMap::new();
    while let Some(cmd) = rx.recv().await {
        jobs.retain(|_, j| !j.done.load(Ordering::Relaxed));
        match cmd {
            Cmd::Fetch { id, path, size } => {
                let Some((job, pc, gathered)) = connection(&id, None, &send).await else {
                    send(Event::Failed { id, begun: false });
                    continue;
                };
                let Ok(dc) = pc.create_data_channel("clip", None).await else {
                    job.fail();
                    continue;
                };
                tokio::spawn(fetch(dc, path, size, job.clone()));
                let sdp = async {
                    let offer = pc.create_offer(None).await.ok()?;
                    pc.set_local_description(offer).await.ok()?;
                    local_sdp(&pc, gathered).await
                }
                .await;
                match sdp {
                    Some(sdp) => send(Event::Sdp { id: id.clone(), sdp, offer: true }),
                    None => job.fail(),
                }
                jobs.insert(id, job);
            }
            Cmd::Accepted { id, sdp } => {
                let Some(job) = jobs.get(&id) else { continue };
                let pc = job.pc.lock().unwrap().clone();
                let Some(pc) = pc else { continue };
                job.alive();
                let ok = match RTCSessionDescription::answer(sdp) {
                    Ok(d) => pc.set_remote_description(d).await.is_ok(),
                    Err(_) => false,
                };
                if !ok {
                    job.fail();
                }
            }
            Cmd::Serve { id, path, offer } => {
                if let Some(old) = jobs.remove(&id) {
                    old.done.store(true, Ordering::Relaxed);
                    old.close();
                }
                let Some((job, pc, gathered)) = connection(&id, Some(path), &send).await else {
                    send(Event::Failed { id, begun: false });
                    continue;
                };
                let sdp = async {
                    pc.set_remote_description(RTCSessionDescription::offer(offer).ok()?).await.ok()?;
                    let answer = pc.create_answer(None).await.ok()?;
                    pc.set_local_description(answer).await.ok()?;
                    local_sdp(&pc, gathered).await
                }
                .await;
                match sdp {
                    Some(sdp) => send(Event::Sdp { id: id.clone(), sdp, offer: false }),
                    None => job.fail(),
                }
                jobs.insert(id, job);
            }
        }
    }
}

async fn connection(id: &str, serve: Option<PathBuf>, send: &Send_) -> Option<(Arc<Job>, Arc<dyn PeerConnection>, tokio::sync::watch::Receiver<bool>)> {
    let mut me = MediaEngine::default();
    let registry = register_default_interceptors(Registry::new(), &mut me).ok()?;
    let config = RTCConfigurationBuilder::new()
        .with_ice_servers(vec![RTCIceServer { urls: ICE.iter().map(|s| s.to_string()).collect(), ..Default::default() }])
        .build();
    let (gathered, gathered_rx) = tokio::sync::watch::channel(false);
    let job = Arc::new(Job {
        id: id.to_string(),
        send: send.clone(),
        pc: Mutex::new(None),
        done: AtomicBool::new(false),
        begun: AtomicBool::new(false),
        last: AtomicU64::new(0),
        at: Instant::now(),
        gathered,
        serve,
    });
    let pc = PeerConnectionBuilder::new()
        .with_configuration(config)
        .with_media_engine(me)
        .with_interceptor_registry(registry)
        .with_handler(Arc::new(Handler { job: job.clone() }) as Arc<dyn PeerConnectionEventHandler>)
        .with_udp_addrs(vec!["0.0.0.0:0"])
        // sending waits once this much is queued: a whole clip isn't held in memory
        .with_data_channel_send_buffer_limit(4 * 1024 * 1024)
        .build()
        .await
        .ok()?;
    let pc: Arc<dyn PeerConnection> = Arc::new(pc);
    *job.pc.lock().unwrap() = Some(pc.clone());
    // the watchdog: no sign of life means the end, whichever side went quiet
    let w = job.clone();
    tokio::spawn(async move {
        while !w.done.load(Ordering::Relaxed) {
            tokio::time::sleep(Duration::from_secs(2)).await;
            if w.at.elapsed().as_millis() as u64 - w.last.load(Ordering::Relaxed) > IDLE.as_millis() as u64 {
                w.fail();
            }
        }
    });
    Some((job, pc, gathered_rx))
}

/// The whole description, candidates included, so one chat message carries it.
async fn local_sdp(pc: &Arc<dyn PeerConnection>, mut gathered: tokio::sync::watch::Receiver<bool>) -> Option<String> {
    let _ = tokio::time::timeout(Duration::from_secs(3), async {
        while !*gathered.borrow() {
            if gathered.changed().await.is_err() {
                break;
            }
        }
    })
    .await;
    pc.local_description().await.map(|d| d.sdp)
}

/// Receiving: bytes go to a `.part` file that becomes the real one when all of them came.
async fn fetch(dc: Arc<dyn DataChannel>, path: PathBuf, size: u64, job: Arc<Job>) {
    let part = path.with_extension("part");
    let mut file = None;
    let mut got = 0u64;
    while let Some(ev) = dc.poll().await {
        match ev {
            DataChannelEvent::OnMessage(m) => {
                job.alive();
                if m.is_string {
                    if &m.data[..] != b"end" {
                        continue;
                    }
                    drop(file.take());
                    let done = got == size && std::fs::rename(&part, &path).is_ok();
                    if done {
                        let _ = dc.send_text("ok").await;
                        // the "ok" leaves before the connection goes
                        tokio::time::sleep(Duration::from_millis(300)).await;
                        job.finish(Event::Received { id: job.id.clone(), path: path.clone() });
                    } else {
                        let _ = std::fs::remove_file(&part);
                        job.fail();
                    }
                    return;
                }
                if file.is_none() {
                    if let Some(dir) = part.parent() {
                        let _ = std::fs::create_dir_all(dir);
                    }
                    match std::fs::File::create(&part) {
                        Ok(f) => file = Some(f),
                        Err(e) => {
                            eprintln!("transfer: {e}");
                            return job.fail();
                        }
                    }
                    job.begun.store(true, Ordering::Relaxed);
                }
                got += m.data.len() as u64;
                // more than was promised isn't what was promised
                let written = got <= size && file.as_mut().is_some_and(|f| f.write_all(&m.data).is_ok());
                if !written {
                    drop(file.take());
                    let _ = std::fs::remove_file(&part);
                    return job.fail();
                }
            }
            DataChannelEvent::OnClose => break,
            _ => {}
        }
        if job.done.load(Ordering::Relaxed) {
            break;
        }
    }
    drop(file.take());
    let _ = std::fs::remove_file(&part);
    job.fail();
}

/// Sending: the file, then "end"; it's delivered when "ok" comes back.
async fn serve(dc: Arc<dyn DataChannel>, path: PathBuf, job: Arc<Job>) {
    let mut started = false;
    while let Some(ev) = dc.poll().await {
        match ev {
            DataChannelEvent::OnOpen if !started => {
                started = true;
                tokio::spawn(push_file(dc.clone(), path.clone(), job.clone()));
            }
            DataChannelEvent::OnMessage(m) if m.is_string && &m.data[..] == b"ok" => {
                return job.finish(Event::Sent { id: job.id.clone() });
            }
            DataChannelEvent::OnClose => break,
            _ => {}
        }
    }
    job.fail();
}

async fn push_file(dc: Arc<dyn DataChannel>, path: PathBuf, job: Arc<Job>) {
    if !send_all(&dc, &path, &job).await {
        job.fail();
    }
}

async fn send_all(dc: &Arc<dyn DataChannel>, path: &Path, job: &Job) -> bool {
    let Ok(mut f) = std::fs::File::open(path) else { return false };
    job.begun.store(true, Ordering::Relaxed);
    let mut buf = vec![0u8; CHUNK];
    loop {
        if job.done.load(Ordering::Relaxed) {
            return false;
        }
        let n = match f.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(_) => return false,
        };
        if dc.send(BytesMut::from(&buf[..n])).await.is_err() {
            return false;
        }
        job.alive();
    }
    dc.send_text("end").await.is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Two engines in one process, over real ICE on this PC: a file gets across intact.
    #[test]
    fn a_file_gets_across() {
        let dir = std::env::temp_dir().join(format!("otium-transfer-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("clip.mp4");
        let dst = dir.join("got.mp4");
        let data: Vec<u8> = (0..6_000_000u32).map(|i| (i.wrapping_mul(2654435761) >> 24) as u8).collect();
        std::fs::write(&src, &data).unwrap();
        let (atx, arx) = std::sync::mpsc::channel::<Event>();
        let (btx, brx) = std::sync::mpsc::channel::<Event>();
        let (atx, btx) = (Mutex::new(atx), Mutex::new(btx));
        let a = Transfer::start(move |e| {
            let _ = atx.lock().unwrap().send(e);
        });
        let b = Transfer::start(move |e| {
            let _ = btx.lock().unwrap().send(e);
        });
        let t = Instant::now();
        a.send(Cmd::Fetch { id: "x".into(), path: dst.clone(), size: data.len() as u64 });
        let wait = |rx: &std::sync::mpsc::Receiver<Event>, f: &dyn Fn(&Event) -> bool| -> Event {
            let end = Instant::now() + Duration::from_secs(60);
            while Instant::now() < end {
                if let Ok(e) = rx.recv_timeout(Duration::from_millis(100)) {
                    assert!(!matches!(e, Event::Failed { .. }), "failed: {e:?}");
                    if f(&e) {
                        return e;
                    }
                }
            }
            panic!("timed out");
        };
        let Event::Sdp { sdp, .. } = wait(&arx, &|e| matches!(e, Event::Sdp { offer: true, .. })) else { unreachable!() };
        b.send(Cmd::Serve { id: "x".into(), path: src, offer: sdp });
        let Event::Sdp { sdp, .. } = wait(&brx, &|e| matches!(e, Event::Sdp { offer: false, .. })) else { unreachable!() };
        a.send(Cmd::Accepted { id: "x".into(), sdp });
        wait(&arx, &|e| matches!(e, Event::Received { .. }));
        wait(&brx, &|e| matches!(e, Event::Sent { .. }));
        println!("{} MB in {:.1?}", data.len() / 1_000_000, t.elapsed());
        assert_eq!(std::fs::read(&dst).unwrap(), data);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn more_than_promised_is_refused() {
        let dir = std::env::temp_dir().join(format!("otium-transfer-short-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("clip.mp4");
        let dst = dir.join("got.mp4");
        std::fs::write(&src, vec![7u8; 100_000]).unwrap();
        let (atx, arx) = std::sync::mpsc::channel::<Event>();
        let (btx, brx) = std::sync::mpsc::channel::<Event>();
        let (atx, btx) = (Mutex::new(atx), Mutex::new(btx));
        let a = Transfer::start(move |e| {
            let _ = atx.lock().unwrap().send(e);
        });
        let b = Transfer::start(move |e| {
            let _ = btx.lock().unwrap().send(e);
        });
        a.send(Cmd::Fetch { id: "x".into(), path: dst.clone(), size: 1000 });
        let Ok(Event::Sdp { sdp, .. }) = arx.recv_timeout(Duration::from_secs(20)) else { panic!("no offer") };
        b.send(Cmd::Serve { id: "x".into(), path: src, offer: sdp });
        let Ok(Event::Sdp { sdp, .. }) = brx.recv_timeout(Duration::from_secs(20)) else { panic!("no answer") };
        a.send(Cmd::Accepted { id: "x".into(), sdp });
        let end = Instant::now() + Duration::from_secs(30);
        let mut failed = false;
        while Instant::now() < end && !failed {
            if let Ok(e) = arx.recv_timeout(Duration::from_millis(200)) {
                assert!(!matches!(e, Event::Received { .. }), "took it");
                failed = matches!(e, Event::Failed { begun: true, .. });
            }
        }
        assert!(failed, "no failure reported");
        assert!(!dst.exists() && !dst.with_extension("part").exists(), "something was left behind");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

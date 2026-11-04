//! Voice: WebRTC audio straight from PC to PC, the same way the Electron build does it, so calls
//! work between both versions. Opus over SRTP, a `state` data channel carrying {muted, deafened},
//! offers and answers sent whole (no trickle ICE) over the chat.
//!
//! One engine handles any number of peers (a 1:1 call is one peer, a group voice room is a mesh).
//! The microphone is captured once, cleaned up with RNNoise, encoded once and sent to every peer;
//! every peer's audio is decoded and mixed into the speakers.

use bytes::Bytes;
use rodio::cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use rtc::interceptor::Registry;
use rtc::media::Sample;
use rtc::media_stream::MediaStreamTrack;
use rtc::peer_connection::configuration::interceptor_registry::register_default_interceptors;
use rtc::peer_connection::configuration::media_engine::{MediaEngine, MIME_TYPE_OPUS};
use rtc::peer_connection::configuration::RTCConfigurationBuilder;
use rtc::peer_connection::sdp::RTCSessionDescription;
use rtc::peer_connection::transport::RTCIceServer;
use rtc::rtp_transceiver::rtp_sender::{RTCRtpCodec, RTCRtpCodecParameters, RTCRtpCodingParameters, RTCRtpEncodingParameters, RtpCodecKind};
use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use webrtc::data_channel::{DataChannel, DataChannelEvent};
use webrtc::media_stream::track_local::static_sample::TrackLocalStaticSample;
use webrtc::media_stream::track_local::TrackLocal;
use webrtc::media_stream::track_remote::{TrackRemote, TrackRemoteEvent};
use webrtc::peer_connection::{PeerConnection, PeerConnectionBuilder, PeerConnectionEventHandler, RTCIceGatheringState, RTCPeerConnectionState};

const RATE: u32 = 48_000;
/// 20 ms of mono audio at 48 kHz
const FRAME: usize = 960;
const ICE: [&str; 2] = ["stun:stun.l.google.com:19302", "stun:stun.cloudflare.com:3478"];

#[derive(Debug)]
pub enum Cmd {
    /// make an offer for `peer` (we call / we're the smaller key in a room)
    Offer(String),
    /// answer `peer`'s offer
    Answer(String, String),
    /// `peer` answered our offer
    Accepted(String, String),
    Close(String),
    CloseAll,
    Muted(bool),
    Deafened(bool),
    /// friends' volume, 0.0 ..= 2.0
    Volume(f32),
    Noise(bool),
}

#[derive(Debug, Clone)]
pub enum Event {
    /// our offer / answer for `peer`, ready to send over the chat
    Sdp { peer: String, sdp: String, offer: bool },
    Connected(String),
    Failed(String),
    Closed(String),
    RemoteState { peer: String, muted: bool, deafened: bool },
    /// who is talking right now (peer key -> speaking); "" is us
    Speaking(HashMap<String, bool>),
    MicError(String),
}

pub struct Voice {
    tx: UnboundedSender<Cmd>,
}

impl Voice {
    pub fn start(send: impl Fn(Event) + Send + Sync + 'static) -> Self {
        let (tx, rx) = unbounded_channel();
        std::thread::Builder::new()
            .name("voice".into())
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

type Send_ = Arc<dyn Fn(Event) + Send + Sync>;

/// Shared between the audio callbacks, the encoder and the peers.
struct Shared {
    muted: AtomicBool,
    deafened: AtomicBool,
    /// volume * 1000
    volume: AtomicU32,
    noise: AtomicBool,
    /// decoded audio per peer, 48 kHz mono, waiting for the speakers
    play: Mutex<HashMap<String, VecDeque<f32>>>,
    /// recent loudness per peer (and "" for us), for the speaking indicators
    level: Mutex<HashMap<String, f32>>,
}

struct Peer {
    pc: Arc<dyn PeerConnection>,
    track: Arc<TrackLocalStaticSample>,
    ssrc: u32,
    pt: u8,
    dc: Arc<Mutex<Option<Arc<dyn DataChannel>>>>,
}

struct Engine {
    shared: Arc<Shared>,
    peers: Arc<tokio::sync::Mutex<HashMap<String, Peer>>>,
    send: Send_,
    audio: Option<AudioIo>,
}

async fn run(mut rx: UnboundedReceiver<Cmd>, send: Send_) {
    let shared = Arc::new(Shared {
        muted: AtomicBool::new(false),
        deafened: AtomicBool::new(false),
        volume: AtomicU32::new(1000),
        noise: AtomicBool::new(true),
        play: Mutex::new(HashMap::new()),
        level: Mutex::new(HashMap::new()),
    });
    let mut e = Engine { shared, peers: Default::default(), send, audio: None };
    let mut levels = tokio::time::interval(Duration::from_millis(120));
    let mut last_speaking: HashMap<String, bool> = HashMap::new();
    loop {
        tokio::select! {
            cmd = rx.recv() => {
                let Some(cmd) = cmd else { return };
                e.handle(cmd).await;
            }
            _ = levels.tick() => {
                // speaking = loud enough recently; only report changes
                let now: HashMap<String, bool> = e.shared.level.lock().unwrap().iter().map(|(k, v)| (k.clone(), *v > 0.02)).collect();
                if now != last_speaking {
                    last_speaking = now.clone();
                    (e.send)(Event::Speaking(now));
                }
                for v in e.shared.level.lock().unwrap().values_mut() {
                    *v *= 0.6;
                }
            }
        }
    }
}

impl Engine {
    async fn handle(&mut self, cmd: Cmd) {
        match cmd {
            Cmd::Offer(peer) => {
                if !self.ensure_audio() {
                    return;
                }
                match self.new_peer(&peer, true).await {
                    Some((pc, gathered)) => {
                        let sdp = async {
                            let offer = pc.create_offer(None).await.ok()?;
                            pc.set_local_description(offer).await.ok()?;
                            local_sdp(&pc, gathered).await
                        }
                        .await;
                        match sdp {
                            Some(sdp) => (self.send)(Event::Sdp { peer, sdp, offer: true }),
                            None => self.fail(&peer).await,
                        }
                    }
                    None => (self.send)(Event::Failed(peer)),
                }
            }
            Cmd::Answer(peer, offer) => {
                if !self.ensure_audio() {
                    return;
                }
                self.close(&peer).await;
                let Some((pc, gathered)) = self.new_peer(&peer, false).await else { return (self.send)(Event::Failed(peer)) };
                let sdp = async {
                    pc.set_remote_description(RTCSessionDescription::offer(offer).ok()?).await.ok()?;
                    let answer = pc.create_answer(None).await.ok()?;
                    pc.set_local_description(answer).await.ok()?;
                    local_sdp(&pc, gathered).await
                }
                .await;
                match sdp {
                    Some(sdp) => (self.send)(Event::Sdp { peer, sdp, offer: false }),
                    None => self.fail(&peer).await,
                }
            }
            Cmd::Accepted(peer, answer) => {
                let pc = self.peers.lock().await.get(&peer).map(|p| p.pc.clone());
                let Some(pc) = pc else { return };
                let ok = match RTCSessionDescription::answer(answer) {
                    Ok(d) => pc.set_remote_description(d).await.is_ok(),
                    Err(_) => false,
                };
                if !ok {
                    self.fail(&peer).await;
                }
            }
            Cmd::Close(peer) => {
                self.close(&peer).await;
                if self.peers.lock().await.is_empty() {
                    self.audio = None; // releases the microphone
                }
            }
            Cmd::CloseAll => {
                let keys: Vec<String> = self.peers.lock().await.keys().cloned().collect();
                for k in keys {
                    self.close(&k).await;
                }
                self.audio = None;
            }
            Cmd::Muted(m) => {
                self.shared.muted.store(m, Ordering::Relaxed);
                self.broadcast_state().await;
            }
            Cmd::Deafened(d) => {
                self.shared.deafened.store(d, Ordering::Relaxed);
                self.broadcast_state().await;
            }
            Cmd::Volume(v) => self.shared.volume.store((v.clamp(0.0, 2.0) * 1000.0) as u32, Ordering::Relaxed),
            Cmd::Noise(n) => self.shared.noise.store(n, Ordering::Relaxed),
        }
    }

    fn ensure_audio(&mut self) -> bool {
        if self.audio.is_some() {
            return true;
        }
        // tests run the network side without touching the microphone and speakers
        if std::env::var_os("OTIUM_VOICE_NO_DEVICES").is_some() {
            return true;
        }
        match AudioIo::start(self.shared.clone(), self.peers.clone(), tokio::runtime::Handle::current()) {
            Ok(a) => {
                self.audio = Some(a);
                true
            }
            Err(e) => {
                (self.send)(Event::MicError(e));
                false
            }
        }
    }

    async fn fail(&mut self, peer: &str) {
        self.close(peer).await;
        (self.send)(Event::Failed(peer.to_string()));
    }

    async fn close(&mut self, peer: &str) {
        let p = self.peers.lock().await.remove(peer);
        if let Some(p) = p {
            let _ = p.pc.close().await;
        }
        self.shared.play.lock().unwrap().remove(peer);
        self.shared.level.lock().unwrap().remove(peer);
    }

    async fn broadcast_state(&self) {
        let msg = state_json(&self.shared);
        for p in self.peers.lock().await.values() {
            let dc = p.dc.lock().unwrap().clone();
            if let Some(dc) = dc {
                let _ = dc.send_text(&msg).await;
            }
        }
    }

    async fn new_peer(&mut self, key: &str, offerer: bool) -> Option<(Arc<dyn PeerConnection>, tokio::sync::watch::Receiver<bool>)> {
        let mut me = MediaEngine::default();
        let opus = RTCRtpCodecParameters {
            rtp_codec: RTCRtpCodec {
                mime_type: MIME_TYPE_OPUS.to_owned(),
                clock_rate: RATE,
                channels: 2,
                sdp_fmtp_line: "minptime=10;useinbandfec=1".to_owned(),
                rtcp_feedback: vec![],
            },
            payload_type: 111,
            ..Default::default()
        };
        me.register_codec(opus.clone(), RtpCodecKind::Audio).ok()?;
        let registry = register_default_interceptors(Registry::new(), &mut me).ok()?;
        let config = RTCConfigurationBuilder::new()
            .with_ice_servers(vec![RTCIceServer { urls: ICE.iter().map(|s| s.to_string()).collect(), ..Default::default() }])
            .build();
        let (gathered_tx, gathered_rx) = tokio::sync::watch::channel(false);
        let dc_slot: Arc<Mutex<Option<Arc<dyn DataChannel>>>> = Default::default();
        let handler = Arc::new(Handler {
            key: key.to_string(),
            send: self.send.clone(),
            shared: self.shared.clone(),
            gathered: gathered_tx,
            dc: dc_slot.clone(),
            connected: Arc::new(AtomicBool::new(false)),
        });
        let pc = PeerConnectionBuilder::new()
            .with_configuration(config)
            .with_media_engine(me)
            .with_interceptor_registry(registry)
            .with_handler(handler.clone() as Arc<dyn PeerConnectionEventHandler>)
            .with_udp_addrs(vec!["0.0.0.0:0"])
            .build()
            .await
            .ok()?;
        let pc: Arc<dyn PeerConnection> = Arc::new(pc);

        let ssrc = rand_u32();
        let track = Arc::new(
            TrackLocalStaticSample::new(
                Instant::now(),
                MediaStreamTrack::new(
                    "otium".to_string(),
                    "otium-mic".to_string(),
                    "otium-mic".to_string(),
                    RtpCodecKind::Audio,
                    vec![RTCRtpEncodingParameters {
                        rtp_coding_parameters: RTCRtpCodingParameters { ssrc: Some(ssrc), ..Default::default() },
                        codec: opus.rtp_codec.clone(),
                        ..Default::default()
                    }],
                ),
            )
            .ok()?,
        );
        let sender = pc.add_track(track.clone() as Arc<dyn TrackLocal>).await.ok()?;
        let pt = sender.get_parameters().await.ok().and_then(|p| p.rtp_parameters.codecs.first().map(|c| c.payload_type)).unwrap_or(111);

        if offerer {
            if let Ok(dc) = pc.create_data_channel("state", None).await {
                watch_channel(dc, handler.clone());
            }
        }
        self.peers.lock().await.insert(key.to_string(), Peer { pc: pc.clone(), track, ssrc, pt, dc: dc_slot });
        Some((pc, gathered_rx))
    }
}

/// Waits until ICE gathering is complete (at most 3 s, like the Electron build) and returns the
/// whole local description, candidates included, so one chat message carries it.
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

fn state_json(s: &Shared) -> String {
    serde_json::json!({ "muted": s.muted.load(Ordering::Relaxed), "deafened": s.deafened.load(Ordering::Relaxed) }).to_string()
}

fn rand_u32() -> u32 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(Instant::now().elapsed().as_nanos());
    h.finish() as u32
}

fn watch_channel(dc: Arc<dyn DataChannel>, h: Arc<Handler>) {
    *h.dc.lock().unwrap() = Some(dc.clone());
    tokio::spawn(async move {
        while let Some(ev) = dc.poll().await {
            match ev {
                DataChannelEvent::OnOpen => {
                    let _ = dc.send_text(&state_json(&h.shared)).await;
                }
                DataChannelEvent::OnMessage(m) => {
                    if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&m.data) {
                        (h.send)(Event::RemoteState {
                            peer: h.key.clone(),
                            muted: v["muted"].as_bool().unwrap_or(false),
                            deafened: v["deafened"].as_bool().unwrap_or(false),
                        });
                    }
                }
                DataChannelEvent::OnClose => break,
                _ => {}
            }
        }
    });
}

struct Handler {
    key: String,
    send: Send_,
    shared: Arc<Shared>,
    gathered: tokio::sync::watch::Sender<bool>,
    dc: Arc<Mutex<Option<Arc<dyn DataChannel>>>>,
    connected: Arc<AtomicBool>,
}

#[async_trait::async_trait]
impl PeerConnectionEventHandler for Handler {
    async fn on_ice_gathering_state_change(&self, state: RTCIceGatheringState) {
        if state == RTCIceGatheringState::Complete {
            let _ = self.gathered.send(true);
        }
    }

    async fn on_connection_state_change(&self, state: RTCPeerConnectionState) {
        match state {
            RTCPeerConnectionState::Connected if !self.connected.swap(true, Ordering::Relaxed) => (self.send)(Event::Connected(self.key.clone())),
            RTCPeerConnectionState::Failed => (self.send)(Event::Failed(self.key.clone())),
            RTCPeerConnectionState::Closed => (self.send)(Event::Closed(self.key.clone())),
            _ => {}
        }
    }

    async fn on_data_channel(&self, dc: Arc<dyn DataChannel>) {
        let h = Arc::new(Handler {
            key: self.key.clone(),
            send: self.send.clone(),
            shared: self.shared.clone(),
            gathered: self.gathered.clone(),
            dc: self.dc.clone(),
            connected: self.connected.clone(),
        });
        watch_channel(dc, h);
    }

    async fn on_track(&self, track: Arc<dyn TrackRemote>) {
        let key = self.key.clone();
        let shared = self.shared.clone();
        tokio::spawn(async move {
            let Ok(mut dec) = opus::Decoder::new(RATE, opus::Channels::Mono) else { return };
            let mut pcm = vec![0f32; 5760];
            while let Some(ev) = track.poll().await {
                let TrackRemoteEvent::OnRtpPacket(pkt) = ev else { continue };
                let Ok(n) = dec.decode_float(&pkt.payload, &mut pcm, false) else { continue };
                let frame = &pcm[..n];
                let level = frame.iter().map(|v| v * v).sum::<f32>() / n.max(1) as f32;
                let deaf = shared.deafened.load(Ordering::Relaxed);
                {
                    let mut lv = shared.level.lock().unwrap();
                    let e = lv.entry(key.clone()).or_insert(0.0);
                    *e = e.max(if deaf { 0.0 } else { level.sqrt() });
                }
                let mut play = shared.play.lock().unwrap();
                let q = play.entry(key.clone()).or_default();
                q.extend(frame.iter().copied());
                // never fall more than 300 ms behind: drop the oldest audio
                let max = RATE as usize * 3 / 10;
                if q.len() > max {
                    let extra = q.len() - max;
                    q.drain(..extra);
                }
            }
        });
    }
}

// ---------- microphone and speakers ----------

struct AudioIo {
    _input: rodio::cpal::Stream,
    _output: rodio::cpal::Stream,
}

// cpal streams are tied to the thread that owns them on some backends; on Windows (WASAPI) they
// are safe to keep in the engine.
unsafe impl Send for AudioIo {}

impl AudioIo {
    fn start(shared: Arc<Shared>, peers: Arc<tokio::sync::Mutex<HashMap<String, Peer>>>, rt: tokio::runtime::Handle) -> Result<Self, String> {
        let host = rodio::cpal::default_host();
        let mic = host.default_input_device().ok_or("Нет микрофона — подключите его и проверьте, что он разрешён в Windows")?;
        let spk = host.default_output_device().ok_or("Нет устройства вывода звука")?;
        let in_cfg = mic.default_input_config().map_err(|e| format!("Микрофон недоступен: {e}"))?;
        let out_cfg = spk.default_output_config().map_err(|e| format!("Звук недоступен: {e}"))?;

        // capture: downmix, resample to 48 kHz, cut into 20 ms frames, denoise, encode, send
        let in_ch = in_cfg.channels() as usize;
        let in_rate = in_cfg.sample_rate().0;
        let (frames_tx, frames_rx) = std::sync::mpsc::sync_channel::<Vec<f32>>(16);
        let mut acc: Vec<f32> = Vec::with_capacity(FRAME * 2);
        let mut rs = Resampler::new(in_rate, RATE);
        let err = |e| eprintln!("audio: {e}");
        let input = match in_cfg.sample_format() {
            rodio::cpal::SampleFormat::F32 => mic.build_input_stream(
                &in_cfg.config(),
                move |data: &[f32], _: &_| {
                    let mono: Vec<f32> = data.chunks(in_ch).map(|c| c.iter().sum::<f32>() / in_ch as f32).collect();
                    rs.push(&mono, &mut acc);
                    while acc.len() >= FRAME {
                        let f: Vec<f32> = acc.drain(..FRAME).collect();
                        let _ = frames_tx.try_send(f);
                    }
                },
                err,
                None,
            ),
            rodio::cpal::SampleFormat::I16 => mic.build_input_stream(
                &in_cfg.config(),
                move |data: &[i16], _: &_| {
                    let mono: Vec<f32> = data.chunks(in_ch).map(|c| c.iter().map(|v| *v as f32 / 32768.0).sum::<f32>() / in_ch as f32).collect();
                    rs.push(&mono, &mut acc);
                    while acc.len() >= FRAME {
                        let f: Vec<f32> = acc.drain(..FRAME).collect();
                        let _ = frames_tx.try_send(f);
                    }
                },
                err,
                None,
            ),
            f => return Err(format!("Формат микрофона не поддерживается: {f:?}")),
        }
        .map_err(|e| format!("Не удалось открыть микрофон: {e}"))?;

        let enc_shared = shared.clone();
        std::thread::Builder::new()
            .name("voice-encode".into())
            .spawn(move || {
                let Ok(mut enc) = opus::Encoder::new(RATE, opus::Channels::Mono, opus::Application::Voip) else { return };
                let _ = enc.set_inband_fec(true);
                let mut denoise = nnnoiseless::DenoiseState::new();
                let mut out = vec![0u8; 4000];
                let mut buf = [0f32; 480];
                while let Ok(mut f) = frames_rx.recv() {
                    if enc_shared.noise.load(Ordering::Relaxed) {
                        // RNNoise works on 10 ms frames in 16-bit range
                        for half in f.chunks_mut(480) {
                            let input: Vec<f32> = half.iter().map(|v| v * 32768.0).collect();
                            denoise.process_frame(&mut buf, &input);
                            for (o, v) in half.iter_mut().zip(buf.iter()) {
                                *o = v / 32768.0;
                            }
                        }
                    }
                    let muted = enc_shared.muted.load(Ordering::Relaxed);
                    if muted {
                        f.iter_mut().for_each(|v| *v = 0.0);
                    }
                    let level = (f.iter().map(|v| v * v).sum::<f32>() / FRAME as f32).sqrt();
                    {
                        let mut lv = enc_shared.level.lock().unwrap();
                        let e = lv.entry(String::new()).or_insert(0.0);
                        *e = e.max(level);
                    }
                    let Ok(n) = enc.encode_float(&f, &mut out) else { continue };
                    let data = Bytes::copy_from_slice(&out[..n]);
                    let peers = peers.clone();
                    rt.spawn(async move {
                        let list: Vec<(Arc<TrackLocalStaticSample>, u32, u8)> = peers.lock().await.values().map(|p| (p.track.clone(), p.ssrc, p.pt)).collect();
                        for (track, ssrc, pt) in list {
                            let s = Sample { data: data.clone(), duration: Duration::from_millis(20), ..Sample::new(Instant::now()) };
                            let _ = track.sample_writer(ssrc, pt).write_sample(&s).await;
                        }
                    });
                }
            })
            .map_err(|e| e.to_string())?;

        // playback: mix every peer, apply the volume, resample to the device rate
        let out_ch = out_cfg.channels() as usize;
        let out_rate = out_cfg.sample_rate().0;
        let play = shared.clone();
        let mut ors = Resampler::new(RATE, out_rate);
        let mut pending: VecDeque<f32> = VecDeque::new();
        let mut mix = move |frames: usize| -> Vec<f32> {
            // pull enough 48 kHz audio to produce `frames` device frames
            let need = (frames as f64 * RATE as f64 / out_rate as f64).ceil() as usize + 4;
            let vol = play.volume.load(Ordering::Relaxed) as f32 / 1000.0;
            let deaf = play.deafened.load(Ordering::Relaxed);
            let mut src = vec![0f32; need];
            {
                let mut q = play.play.lock().unwrap();
                for buf in q.values_mut() {
                    // wait for ~60 ms of audio before starting a peer, so small jitter doesn't click
                    if buf.len() < RATE as usize * 6 / 100 && buf.len() < need {
                        continue;
                    }
                    for (i, v) in buf.drain(..need.min(buf.len())).enumerate() {
                        src[i] += v;
                    }
                }
            }
            if deaf {
                src.iter_mut().for_each(|v| *v = 0.0);
            }
            let mut res = Vec::with_capacity(frames + 8);
            ors.push(&src, &mut res);
            pending.extend(res);
            (0..frames).map(|_| (pending.pop_front().unwrap_or(0.0) * vol).clamp(-1.0, 1.0)).collect()
        };
        let output = match out_cfg.sample_format() {
            rodio::cpal::SampleFormat::F32 => spk.build_output_stream(
                &out_cfg.config(),
                move |data: &mut [f32], _: &_| {
                    let m = mix(data.len() / out_ch);
                    for (frame, v) in data.chunks_mut(out_ch).zip(m) {
                        frame.iter_mut().for_each(|s| *s = v);
                    }
                },
                err,
                None,
            ),
            rodio::cpal::SampleFormat::I16 => spk.build_output_stream(
                &out_cfg.config(),
                move |data: &mut [i16], _: &_| {
                    let m = mix(data.len() / out_ch);
                    for (frame, v) in data.chunks_mut(out_ch).zip(m) {
                        frame.iter_mut().for_each(|s| *s = (v * 32767.0) as i16);
                    }
                },
                err,
                None,
            ),
            f => return Err(format!("Формат звука не поддерживается: {f:?}")),
        }
        .map_err(|e| format!("Не удалось открыть звук: {e}"))?;
        input.play().map_err(|e| e.to_string())?;
        output.play().map_err(|e| e.to_string())?;
        Ok(Self { _input: input, _output: output })
    }
}

/// Linear resampler, good enough for voice.
struct Resampler {
    step: f64,
    pos: f64,
    last: f32,
    primed: bool,
}

impl Resampler {
    fn new(from: u32, to: u32) -> Self {
        Self { step: from as f64 / to as f64, pos: 0.0, last: 0.0, primed: false }
    }

    /// The stream seen as [last sample of the previous call, input...]; `pos` indexes into it.
    fn push(&mut self, input: &[f32], out: &mut Vec<f32>) {
        if (self.step - 1.0).abs() < 1e-9 {
            out.extend_from_slice(input);
            return;
        }
        let n = input.len();
        if n == 0 {
            return;
        }
        if !self.primed {
            // start the stream at its first real sample, not at silence
            self.primed = true;
            self.last = input[0];
        }
        let at = |k: usize| if k == 0 { self.last } else { input[k - 1] };
        while self.pos + 1.0 <= n as f64 {
            let i = self.pos.floor() as usize;
            let t = (self.pos - i as f64) as f32;
            let (a, b) = (at(i), at(i + 1));
            out.push(a + (b - a) * t);
            self.pos += self.step;
        }
        self.pos -= n as f64;
        if let Some(l) = input.last() {
            self.last = *l;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resamples() {
        let mut r = Resampler::new(44_100, 48_000);
        let mut out = Vec::new();
        r.push(&vec![0.5; 441], &mut out);
        assert!((out.len() as i64 - 480).abs() <= 1, "{}", out.len());
        assert!(out.iter().all(|v| (*v - 0.5).abs() < 1e-6));
        let mut same = Resampler::new(48_000, 48_000);
        let mut o2 = Vec::new();
        same.push(&[1.0, 2.0], &mut o2);
        assert_eq!(o2, vec![1.0, 2.0]);
    }

    #[test]
    fn opus_round_trip() {
        let mut enc = opus::Encoder::new(RATE, opus::Channels::Mono, opus::Application::Voip).unwrap();
        let mut dec = opus::Decoder::new(RATE, opus::Channels::Mono).unwrap();
        let tone: Vec<f32> = (0..FRAME).map(|i| (i as f32 * 0.05).sin() * 0.5).collect();
        let mut pkt = vec![0u8; 4000];
        let n = enc.encode_float(&tone, &mut pkt).unwrap();
        let mut pcm = vec![0f32; 5760];
        let m = dec.decode_float(&pkt[..n], &mut pcm, false).unwrap();
        assert_eq!(m, FRAME);
    }
}

#[cfg(test)]
mod loopback {
    use super::*;

    /// Two engines in one process call each other over real ICE (host candidates), without
    /// audio devices: the connection comes up and the state channel carries "muted".
    #[test]
    fn two_engines_connect() {
        std::env::set_var("OTIUM_VOICE_NO_DEVICES", "1");
        let (atx, arx) = std::sync::mpsc::channel::<Event>();
        let (btx, brx) = std::sync::mpsc::channel::<Event>();
        let atx = Mutex::new(atx);
        let btx = Mutex::new(btx);
        let a = Voice::start(move |e| { let _ = atx.lock().unwrap().send(e); });
        let b = Voice::start(move |e| { let _ = btx.lock().unwrap().send(e); });
        a.send(Cmd::Offer("b".into()));
        let wait = |rx: &std::sync::mpsc::Receiver<Event>, what: &str, f: &dyn Fn(&Event) -> bool| -> Event {
            let end = Instant::now() + Duration::from_secs(20);
            while Instant::now() < end {
                if let Ok(e) = rx.recv_timeout(Duration::from_millis(200)) {
                    if f(&e) {
                        return e;
                    }
                }
            }
            panic!("timed out waiting for {what}");
        };
        let Event::Sdp { sdp: offer, .. } = wait(&arx, "offer", &|e| matches!(e, Event::Sdp { offer: true, .. })) else { unreachable!() };
        assert!(offer.contains("opus/48000/2"), "offer has no opus");
        assert!(offer.contains("a=candidate"), "offer has no candidates");
        b.send(Cmd::Answer("a".into(), offer));
        let Event::Sdp { sdp: answer, .. } = wait(&brx, "answer", &|e| matches!(e, Event::Sdp { offer: false, .. })) else { unreachable!() };
        a.send(Cmd::Accepted("b".into(), answer));
        wait(&arx, "A connected", &|e| matches!(e, Event::Connected(_)));
        wait(&brx, "B connected", &|e| matches!(e, Event::Connected(_)));
        a.send(Cmd::Muted(true));
        wait(&brx, "muted state at B", &|e| matches!(e, Event::RemoteState { muted: true, .. }));
        println!("connected both ways, state channel works");
    }
}

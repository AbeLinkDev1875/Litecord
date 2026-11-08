//! The engine thread: the open room, its relay connection and the WebRTC connections to
//! everyone in the same voice channel. It works like the web app's `syncMedia`, `makePeer`,
//! `onSignal` and `handleSignal`: one connection per person, "perfect negotiation" (either side
//! may start; the polite one, with the smaller id, gives way on a clash), and a fresh start
//! when a connection fails.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use libwebrtc::media_stream_track::MediaStreamTrack;
use libwebrtc::peer_connection::{AnswerOptions, OfferOptions, PeerConnection, PeerConnectionState, SignalingState};
use libwebrtc::peer_connection_factory::native::PeerConnectionFactoryExt;
use libwebrtc::peer_connection_factory::{IceServer, PeerConnectionFactory, RtcConfiguration};
use libwebrtc::rtp_parameters::RtpEncodingParameters;
use libwebrtc::rtp_sender::RtpSender;
use libwebrtc::rtp_transceiver::{RtpTransceiverDirection, RtpTransceiverInit};
use libwebrtc::session_description::{SdpType, SessionDescription};
use libwebrtc::ice_candidate::IceCandidate;
use libwebrtc::stats::RtcStats;
use serde_json::{json, Value};
use tokio::sync::mpsc::{self, UnboundedSender};

use super::relay::{self, BigIn, RelayIn, RelayOut};
use super::room::{Room, RoomEvent};
use super::video::{self, Frames};
use super::{CallEvent, CallView, ChannelView, ChatView, Cmd, PeerStats, PersonView};

macro_rules! dbg_sig {
    ($($t:tt)*) => { if std::env::var_os("LC_DEBUG").is_some() { eprintln!($($t)*); } };
}

pub enum Ev {
    Cmd(Cmd),
    Relay(u64, RelayIn),
    Neg(String, u64),
    Ice(String, u64, Value),
    Conn(String, u64, PeerConnectionState),
    Track(String, u64, MediaStreamTrack, String, bool),
    Rebuild(String, u64),
    IceServers(Vec<IceServer>),
    Tick,
    Stats,
    /// A video capture stopped by itself (camera unplugged, the user ended a screen share).
    CaptureEnded(&'static str, u64),
}

/// What we send: the mic, the camera, the screen.
/// A source we send (the microphone, camera or screen). Each connection makes its own track
/// from it, in its own factory.
#[derive(Clone)]
pub enum Src {
    Audio(libwebrtc::audio_source::native::NativeAudioSource),
    Video(libwebrtc::video_source::native::NativeVideoSource),
}

struct Local {
    stream_id: String,
    tracks: Vec<Src>,
    enc: Option<RtpEncodingParameters>,
    gen: u64,
}

struct Peer {
    /// Each connection has its own WebRTC factory: LiveKit's shared audio plumbing stops
    /// decoding new incoming audio once every earlier incoming stream has ended, so sharing
    /// one factory would leave people who join later silent.
    factory: PeerConnectionFactory,
    pc: PeerConnection,
    gen: u64,
    polite: bool,
    /// key ("mic", "cam", "screen") -> (local gen, senders)
    out: HashMap<&'static str, (u64, Vec<RtpSender>)>,
    created: Instant,
    rebuilt: Option<Instant>,
    warned: bool,
    state: PeerConnectionState,
    /// We wanted to offer but couldn't yet (in the middle of another exchange).
    want_offer: bool,
}

struct Remote {
    peer: String,
    gen: u64,
    kind: &'static str, // "voice", "screen-audio", "video"
    stream_id: String,
    track: MediaStreamTrack,
    key: Option<String>,
    /// The task reading this person's audio into the speaker mix.
    task: Option<tokio::task::AbortHandle>,
}

struct Engine {
    base: String,
    events: async_channel::Sender<CallEvent>,
    view: Arc<Mutex<CallView>>,
    frames: Frames,
    tx: UnboundedSender<Ev>,
    audio: Option<super::audio::Audio>,
    /// What the audio thread has open: None = nothing, Some(true) = mic and speakers.
    audio_open: Option<bool>,
    mic_source: Option<libwebrtc::audio_source::native::NativeAudioSource>,
    ice: Vec<IceServer>,

    room: Option<Room>,
    room_gen: u64,
    relay: Option<UnboundedSender<RelayOut>>,
    connected: bool,
    pending_voice: Option<String>,
    since: Option<Instant>,
    big: BigIn,

    peers: HashMap<String, Peer>,
    /// Connections we closed lately: if we connect to them again soon, their side may still
    /// hold the old one (the web app keeps it 15 seconds), so we ask them to start over.
    closed_at: HashMap<String, Instant>,
    retries: HashMap<String, u32>,
    remote: Vec<Remote>,
    gen: u64,
    mic: Option<Local>,
    /// The microphone track, made once and reused for every call.
    mic_failed: bool,
    cam: Option<(Local, video::Capture)>,
    screen: Option<(Local, video::Capture)>,
    local_mute: HashSet<String>,
    speaking: HashSet<String>,
    stats: Vec<PeerStats>,
    mic_name: String,
    speaker_name: String,
}

pub fn spawn(base: String, events: async_channel::Sender<CallEvent>, view: Arc<Mutex<CallView>>, frames: Frames) -> UnboundedSender<Ev> {
    let (tx, mut rx) = mpsc::unbounded_channel::<Ev>();
    let me = tx.clone();
    std::thread::Builder::new()
        .name("litecord-call".into())
        .spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().expect("tokio runtime");
            rt.block_on(async move {
                let mut e = Engine {
                    base,
                    events,
                    view,
                    frames,
                    tx: me.clone(),
                    audio: None,
                    audio_open: None,
                    mic_source: None,
                    ice: default_ice(),
                    room: None,
                    room_gen: 0,
                    relay: None,
                    connected: false,
                    pending_voice: None,
                    since: None,
                    big: BigIn::default(),
                    peers: HashMap::new(),
                    closed_at: HashMap::new(),
                    retries: HashMap::new(),
                    remote: Vec::new(),
                    gen: 0,
                    mic: None,
                    mic_failed: false,
                    cam: None,
                    screen: None,
                    local_mute: HashSet::new(),
                    speaking: HashSet::new(),
                    stats: Vec::new(),
                    mic_name: String::new(),
                    speaker_name: String::new(),
                };
                let tick = me.clone();
                tokio::spawn(async move {
                    let mut n = 0u64;
                    loop {
                        tokio::time::sleep(Duration::from_millis(300)).await;
                        n += 1;
                        if tick.send(Ev::Tick).is_err() {
                            return;
                        }
                        if n % 3 == 0 && tick.send(Ev::Stats).is_err() {
                            return;
                        }
                    }
                });
                while let Some(ev) = rx.recv().await {
                    if let Ev::Cmd(Cmd::Quit) = ev {
                        e.close_room();
                        break;
                    }
                    e.handle(ev).await;
                }
            });
        })
        .expect("call thread");
    tx
}

fn default_ice() -> Vec<IceServer> {
    vec![IceServer { urls: vec!["stun:stun.l.google.com:19302".into(), "stun:stun.cloudflare.com:3478".into()], username: String::new(), password: String::new() }]
}

/// Reads `GET /api/ice` like the web app: STUN, plus Cloudflare TURN when the server has it.
fn fetch_ice(base: &str) -> Option<Vec<IceServer>> {
    let mut res = ureq::get(&format!("{}/api/ice", base.trim_end_matches('/'))).call().ok()?;
    let v: Value = res.body_mut().read_json().ok()?;
    let list: Vec<IceServer> = v.get("iceServers")?.as_array()?.iter().filter_map(|s| {
        let urls = match s.get("urls")? {
            Value::String(u) => vec![u.clone()],
            Value::Array(a) => a.iter().filter_map(|u| u.as_str().map(String::from)).collect(),
            _ => return None,
        };
        Some(IceServer {
            urls,
            username: s.get("username").and_then(|u| u.as_str()).unwrap_or("").into(),
            password: s.get("credential").and_then(|u| u.as_str()).unwrap_or("").into(),
        })
    }).collect();
    if list.is_empty() { None } else { Some(list) }
}

fn sdp_json(d: &SessionDescription) -> Value {
    json!({"type": d.sdp_type().to_string(), "sdp": d.to_string()})
}

impl Engine {
    fn emit(&self, ev: CallEvent) {
        let _ = self.events.try_send(ev);
    }

    async fn handle(&mut self, ev: Ev) {
        match ev {
            Ev::Cmd(c) => self.on_cmd(c).await,
            Ev::Relay(g, r) if g == self.room_gen => self.on_relay(r).await,
            Ev::Relay(..) => {}
            Ev::IceServers(list) => self.ice = list,
            Ev::Neg(id, g) => self.on_negotiation_needed(&id, g).await,
            Ev::Ice(id, g, c) => {
                if self.peers.get(&id).map(|p| p.gen) == Some(g) {
                    self.relay_send(&id, &json!({"type": "signal", "data": {"candidate": c}}));
                }
            }
            Ev::Conn(id, g, st) => self.on_conn(&id, g, st),
            Ev::Track(id, g, track, stream_id, has_video) => self.on_track(&id, g, track, stream_id, has_video),
            Ev::Rebuild(id, g) => {
                if self.peers.get(&id).map(|p| p.gen) == Some(g) {
                    self.rebuild_peer(&id);
                }
            }
            Ev::Tick => self.on_tick(),
            Ev::Stats => self.collect_stats().await,
            Ev::CaptureEnded(which, g) => {
                if which == "cam" && self.cam.as_ref().map(|c| c.0.gen) == Some(g) {
                    self.stop_camera();
                } else if which == "screen" && self.screen.as_ref().map(|c| c.0.gen) == Some(g) {
                    self.stop_screen();
                }
            }
        }
        self.publish();
    }

    // ---------- the room ----------

    async fn on_cmd(&mut self, c: Cmd) {
        match c {
            Cmd::Open { room, uid, name, status, custom, saved } => {
                if self.room.as_ref().map(|r| r.name == room).unwrap_or(false) {
                    return;
                }
                self.close_room();
                let my_id: String = format!("{}{}", super::room::rid(4), (super::room::now_ms() as u64 % 1_000_000));
                let mut r = Room::new(&room, &my_id, &uid, &name);
                r.status = status;
                r.custom = custom;
                if let Some(s) = saved {
                    r.load_saved(&s);
                }
                self.room = Some(r);
                self.room_gen += 1;
                let (otx, orx) = mpsc::unbounded_channel::<RelayOut>();
                self.relay = Some(otx);
                let (itx, mut irx) = mpsc::unbounded_channel::<(u64, RelayIn)>();
                tokio::spawn(relay::run(relay::url(&self.base, &room, &my_id, &name), orx, itx, self.room_gen));
                let fwd = self.tx.clone();
                tokio::spawn(async move {
                    while let Some((g, m)) = irx.recv().await {
                        if fwd.send(Ev::Relay(g, m)).is_err() {
                            return;
                        }
                    }
                });
                let base = self.base.clone();
                let tx = self.tx.clone();
                tokio::task::spawn_blocking(move || {
                    if let Some(list) = fetch_ice(&base) {
                        let _ = tx.send(Ev::IceServers(list));
                    }
                });
            }
            Cmd::Close => self.close_room(),
            Cmd::JoinVoice(ch) => {
                self.pending_voice = Some(ch);
                self.try_join_voice();
            }
            Cmd::LeaveVoice => self.leave_voice(),
            Cmd::SetMuted(m) => {
                if let Some(r) = self.room.as_mut() {
                    r.me.muted = m;
                    if !m && r.me.deaf {
                        r.me.deaf = false;
                    }
                    r.send_state();
                }
                self.emit(CallEvent::Sound(if m { "mute" } else { "unmute" }));
                self.apply_mic_enabled();
                self.apply_audio();
                self.flush();
            }
            Cmd::SetDeaf(d) => {
                if let Some(r) = self.room.as_mut() {
                    r.me.deaf = d;
                    r.send_state();
                }
                self.emit(CallEvent::Sound(if d { "deafen" } else { "undeafen" }));
                self.apply_mic_enabled();
                self.apply_audio();
                self.flush();
            }
            Cmd::Camera(on) => {
                if on { self.start_camera() } else { self.stop_camera() }
            }
            Cmd::Screen(Some(src)) => self.start_screen(src),
            Cmd::Screen(None) => self.stop_screen(),
            Cmd::LocalMute(id, m) => {
                if m { self.local_mute.insert(id); } else { self.local_mute.remove(&id); }
                self.apply_audio();
            }
            Cmd::Chat { ch, text } => {
                if let Some(r) = self.room.as_mut() {
                    r.chat(&ch, &text);
                }
                self.flush();
            }
            Cmd::Typing(ch) => {
                if let Some(r) = self.room.as_mut() {
                    r.send(json!({"type": "typing", "ch": ch}));
                }
                self.flush();
            }
            Cmd::Status { status, custom } => {
                if let Some(r) = self.room.as_mut() {
                    r.status = status.clone();
                    r.custom = custom.clone();
                    r.send(json!({"type": "status", "status": status, "custom": custom, "pf": r.pf}));
                }
                self.flush();
            }
            Cmd::Rename(n) => {
                if let Some(r) = self.room.as_mut() {
                    r.my_name = n.clone();
                    r.send(json!({"type": "rename", "name": n}));
                }
                self.flush();
            }
            Cmd::Devices { mic, speaker } => {
                self.mic_name = mic;
                self.speaker_name = speaker;
                if self.audio_open.is_some() {
                    self.audio_open = None; // reopen with the new devices
                    self.update_mic();
                }
            }
            Cmd::Quit => {}
        }
    }

    fn close_room(&mut self) {
        if self.room.as_ref().map(|r| r.me.voice.is_some()).unwrap_or(false) {
            self.leave_voice();
        }
        self.save_room();
        for id in self.peers.keys().cloned().collect::<Vec<_>>() {
            self.close_peer(&id);
        }
        self.relay = None; // drops the socket
        self.room = None;
        self.room_gen += 1;
        self.connected = false;
        self.pending_voice = None;
        self.big = BigIn::default();
    }

    fn save_room(&mut self) {
        if let Some(r) = self.room.as_mut() {
            if r.dirty && !r.is_dm_room() && r.have_data {
                r.dirty = false;
                let _ = self.events.try_send(CallEvent::Save(r.name.clone(), r.snapshot()));
            }
        }
    }

    fn try_join_voice(&mut self) {
        let Some(ch) = self.pending_voice.clone() else { return };
        let Some(r) = self.room.as_mut() else { return };
        if !r.ready || r.ch_by_id(&ch).is_none() {
            return; // joins once the channel list is here
        }
        self.pending_voice = None;
        if r.me.voice.as_deref() == Some(ch.as_str()) {
            return;
        }
        r.join_voice(&ch);
        self.since = Some(Instant::now());
        self.mic_failed = false;
        self.emit(CallEvent::Sound("join"));
        self.update_mic();
        self.flush();
        self.sync_media();
    }

    fn leave_voice(&mut self) {
        let Some(r) = self.room.as_mut() else { return };
        if r.me.voice.is_none() {
            self.pending_voice = None;
            return;
        }
        r.leave_voice();
        self.pending_voice = None;
        self.since = None;
        self.emit(CallEvent::Sound("leave"));
        self.stop_camera();
        self.stop_screen();
        self.update_mic();
        self.flush();
        self.sync_media();
    }

    async fn on_relay(&mut self, m: RelayIn) {
        match m {
            RelayIn::Open => {
                self.connected = true;
                if let Some(r) = self.room.as_mut() {
                    r.socket_opened();
                }
            }
            RelayIn::Closed => {
                self.connected = false;
                if let Some(r) = self.room.as_mut() {
                    r.ready = false;
                }
            }
            RelayIn::Text(t) => {
                let Ok(m) = serde_json::from_str::<Value>(&t) else { return };
                match m.get("type").and_then(|t| t.as_str()) {
                    Some("peers") => {
                        if let Some(r) = self.room.as_mut() {
                            r.on_peers(&m);
                        }
                        let present = self.room.as_ref().map(|r| r.present.clone()).unwrap_or_default();
                        for id in self.peers.keys().cloned().collect::<Vec<_>>() {
                            if !present.contains(&id) {
                                self.close_peer(&id);
                            }
                        }
                        self.after_room();
                    }
                    Some("msg") => {
                        let from = m.get("from").and_then(|f| f.as_str()).unwrap_or("").to_string();
                        let data = m.get("data").cloned().unwrap_or(Value::Null);
                        self.on_peer_text(&from, data).await;
                    }
                    _ => {}
                }
            }
            RelayIn::Binary(b) => {
                let Some((from, payload)) = relay::split_from(&b) else { return };
                if let Some(text) = self.big.add(&from, payload) {
                    if let Ok(v) = serde_json::from_str::<Value>(&text) {
                        self.on_peer_text(&from, v).await;
                    }
                }
            }
        }
    }

    async fn on_peer_text(&mut self, from: &str, m: Value) {
        match m.get("type").and_then(|t| t.as_str()) {
            Some("signal") => {
                let in_voice = self.room.as_ref().map(|r| r.me.voice.is_some()).unwrap_or(false);
                if in_voice {
                    self.on_signal(from, m.get("data").cloned().unwrap_or(Value::Null)).await;
                }
            }
            Some("dsig") => {} // data links: we talk to the host through the relay instead
            _ => {
                if let Some(r) = self.room.as_mut() {
                    r.on_peer_text(from, &m);
                }
                self.after_room();
            }
        }
    }

    /// After the room changed: send what it wants to send, and react to its events.
    fn after_room(&mut self) {
        self.flush();
        let evs = self.room.as_mut().map(|r| std::mem::take(&mut r.events)).unwrap_or_default();
        let mut media = false;
        for e in evs {
            match e {
                RoomEvent::People | RoomEvent::Synced => media = true,
                RoomEvent::LeftVoice => {
                    media = true;
                    self.stop_camera();
                    self.stop_screen();
                    self.update_mic();
                }
                RoomEvent::Sound(s) => self.emit(CallEvent::Sound(s)),
                RoomEvent::Toast(t) => self.emit(CallEvent::Toast(t)),
                RoomEvent::Chat(ch) => self.emit(CallEvent::Chat(ch)),
            }
        }
        // The host decides who's a speaker on a stage: stop sending if we're not one.
        let (in_voice, can) = self.room.as_ref().map(|r| (r.me.voice.is_some(), r.can_send())).unwrap_or((false, false));
        if in_voice && !can {
            self.stop_camera();
            self.stop_screen();
        }
        if in_voice && can != self.mic.is_some() && !self.mic_failed {
            self.update_mic();
        }
        self.try_join_voice();
        if media {
            self.sync_media();
        }
        if self.room.as_ref().map(|r| r.dirty).unwrap_or(false) {
            self.save_room();
        }
        self.flush();
    }

    /// Passes the room's outgoing messages to the relay.
    fn flush(&mut self) {
        let out = self.room.as_mut().map(|r| std::mem::take(&mut r.out)).unwrap_or_default();
        for (to, text) in out {
            self.relay_raw(&to, &text);
        }
    }

    fn relay_raw(&mut self, to: &str, text: &str) {
        let Some(relay) = self.relay.as_ref() else { return };
        if text.len() <= 16000 {
            let _ = relay.send(RelayOut::Text(format!("{{\"to\":{},\"data\":{}}}", json!(to), text)));
            return;
        }
        // Big messages go in pieces, like the web app's sendText.
        let bytes = text.as_bytes();
        let mid = super::room::rid(6);
        let chunks: Vec<&[u8]> = bytes.chunks(16 * 1024).collect();
        for (i, c) in chunks.iter().enumerate() {
            let fr = relay::frame(&json!({"t": "big", "mid": mid, "i": i, "n": chunks.len()}), c);
            let _ = relay.send(RelayOut::Binary(relay::addressed(to, &fr)));
        }
    }

    fn relay_send(&mut self, to: &str, obj: &Value) {
        self.relay_raw(to, &obj.to_string());
    }

    // ---------- media ----------

    fn audio(&mut self) -> &super::audio::Audio {
        self.audio.get_or_insert_with(super::audio::Audio::new)
    }

    /// Opens the microphone (when we may speak) and speakers while we're in a call, and
    /// closes them when we leave.
    fn update_mic(&mut self) {
        let in_voice = self.room.as_ref().map(|r| r.me.voice.is_some()).unwrap_or(false);
        let want = self.room.as_ref().map(|r| r.can_send()).unwrap_or(false) && !self.mic_failed;
        if want && self.mic.is_none() {
            self.gen += 1;
            let src = self.mic_source.get_or_insert_with(super::audio::mic_source).clone();
            self.mic = Some(Local { stream_id: format!("mic-{}", super::room::rid(6)), tracks: vec![Src::Audio(src)], enc: None, gen: self.gen });
        } else if !want && self.mic.is_some() {
            self.mic = None;
        }
        let open = if in_voice { Some(self.mic.is_some()) } else { None };
        if open != self.audio_open {
            self.audio_open = open;
            let (mic, spk, src) = (self.mic_name.clone(), self.speaker_name.clone(), self.mic_source.clone());
            match open {
                Some(with_mic) => self.audio().start(&mic, &spk, if with_mic { src } else { None }),
                None => self.audio().stop(),
            }
        }
        self.apply_mic_enabled();
    }

    fn apply_mic_enabled(&mut self) {
        let on = self.room.as_ref().map(|r| !r.me.muted && !r.me.deaf).unwrap_or(false);
        if let Some(a) = &self.audio {
            a.shared.mic_off.store(!on, std::sync::atomic::Ordering::Relaxed);
        }
    }

    fn sync_media(&mut self) {
        let Some(r) = self.room.as_ref() else { return };
        let voice = r.me.voice.clone();
        let present = r.present.clone();
        let close: Vec<String> = self.peers.iter().filter(|(id, p)| {
            !r.same_call(id) && (voice.is_none() || p.created.elapsed() > Duration::from_secs(15) || !present.contains(*id))
        }).map(|(id, _)| id.clone()).collect();
        let want_ids: Vec<String> = r.people.keys().filter(|id| r.same_call(id)).cloned().collect();
        let ok = r.can_send();
        for id in close {
            self.close_peer(&id);
        }
        for id in want_ids {
            let have_any = ok && (self.mic.is_some() || self.cam.is_some() || self.screen.is_some());
            if !self.peers.contains_key(&id) && !have_any {
                continue;
            }
            if !self.peers.contains_key(&id) {
                if !self.make_peer(&id) {
                    continue;
                }
                if self.closed_at.get(&id).is_some_and(|t| t.elapsed() < Duration::from_secs(20)) {
                    self.relay_send(&id, &json!({"type": "signal", "data": {"reset": true}}));
                    if let Some(p) = self.peers.get_mut(&id) {
                        p.rebuilt = Some(Instant::now());
                    }
                }
            }
            let mic = if ok { self.mic.as_ref().map(|l| (l.gen, l.stream_id.clone(), l.tracks.clone(), l.enc.clone())) } else { None };
            let cam = if ok { self.cam.as_ref().map(|l| (l.0.gen, l.0.stream_id.clone(), l.0.tracks.clone(), l.0.enc.clone())) } else { None };
            let scr = if ok { self.screen.as_ref().map(|l| (l.0.gen, l.0.stream_id.clone(), l.0.tracks.clone(), l.0.enc.clone())) } else { None };
            let p = self.peers.get_mut(&id).unwrap();
            set_stream(p, "mic", mic);
            set_stream(p, "cam", cam);
            set_stream(p, "screen", scr);
        }
        self.apply_audio();
    }

    fn make_peer(&mut self, id: &str) -> bool {
        let mut cfg = RtcConfiguration::default();
        cfg.ice_servers = self.ice.clone();
        let factory = PeerConnectionFactory::default();
        let pc = match factory.create_peer_connection(cfg) {
            Ok(pc) => pc,
            Err(e) => {
                eprintln!("couldn't make a connection: {e:?}");
                return false;
            }
        };
        self.gen += 1;
        let gen = self.gen;
        let my_id = self.room.as_ref().map(|r| r.my_id.clone()).unwrap_or_default();
        let (tx, pid) = (self.tx.clone(), id.to_string());
        pc.on_negotiation_needed(Some(Box::new(move |_| { let _ = tx.send(Ev::Neg(pid.clone(), gen)); })));
        let (tx, pid) = (self.tx.clone(), id.to_string());
        pc.on_ice_candidate(Some(Box::new(move |c: IceCandidate| {
            let _ = tx.send(Ev::Ice(pid.clone(), gen, json!({"candidate": c.candidate(), "sdpMid": c.sdp_mid(), "sdpMLineIndex": c.sdp_mline_index()})));
        })));
        let (tx, pid) = (self.tx.clone(), id.to_string());
        pc.on_connection_state_change(Some(Box::new(move |st| { let _ = tx.send(Ev::Conn(pid.clone(), gen, st)); })));
        let (tx, pid) = (self.tx.clone(), id.to_string());
        pc.on_track(Some(Box::new(move |e: libwebrtc::peer_connection::TrackEvent| {
            let (sid, has_video) = e.streams.first().map(|s| (s.id(), !s.video_tracks().is_empty())).unwrap_or_default();
            let _ = tx.send(Ev::Track(pid.clone(), gen, e.track, sid, has_video));
        })));
        self.peers.insert(id.to_string(), Peer {
            factory,
            pc,
            gen,
            polite: my_id.as_str() < id,
            out: HashMap::new(),
            created: Instant::now(),
            rebuilt: None,
            warned: false,
            state: PeerConnectionState::New,
            want_offer: false,
        });
        true
    }

    fn close_peer(&mut self, id: &str) {
        if let Some(p) = self.peers.remove(id) {
            self.closed_at.insert(id.to_string(), Instant::now());
            p.pc.on_negotiation_needed(None);
            p.pc.on_ice_candidate(None);
            p.pc.on_connection_state_change(None);
            p.pc.on_track(None);
            p.pc.close();
        }
        let (gone, keep): (Vec<Remote>, Vec<Remote>) = std::mem::take(&mut self.remote).into_iter().partition(|r| r.peer == id);
        self.remote = keep;
        for rm in gone {
            self.drop_remote(rm);
        }
    }

    fn drop_remote(&mut self, rm: Remote) {
        if let Some(t) = rm.task {
            t.abort();
        }
        if let Some(k) = rm.key {
            video::stop_remote(&self.frames, &k);
            if let Some(a) = &self.audio {
                a.remove_remote(&k);
            }
        }
    }

    fn rebuild_peer(&mut self, id: &str) {
        self.close_peer(id);
        self.relay_send(id, &json!({"type": "signal", "data": {"reset": true}}));
        self.sync_media();
        if let Some(p) = self.peers.get_mut(id) {
            p.rebuilt = Some(Instant::now());
        }
    }

    async fn on_negotiation_needed(&mut self, id: &str, g: u64) {
        let Some(p) = self.peers.get_mut(id) else { return };
        if p.gen != g {
            return;
        }
        dbg_sig!("[{id}] negotiation needed in {:?}", p.pc.signaling_state());
        p.want_offer = true;
        self.maybe_offer(id).await;
    }

    /// Makes an offer if one is wanted and we're not in the middle of another exchange.
    /// libwebrtc only says "negotiation needed" once, so a wish that came at a bad moment
    /// (or tracks left over from a clash) is remembered and done here later.
    async fn maybe_offer(&mut self, id: &str) {
        let Some(p) = self.peers.get_mut(id) else { return };
        if p.pc.signaling_state() != SignalingState::Stable {
            return;
        }
        let loose = p.pc.transceivers().iter().any(|t| t.mid().is_none() && t.direction() == RtpTransceiverDirection::SendOnly);
        if !p.want_offer && !loose {
            return;
        }
        p.want_offer = false;
        let (pc, g) = (p.pc.clone(), p.gen);
        let offer = match pc.create_offer(OfferOptions { ice_restart: false, offer_to_receive_audio: None, offer_to_receive_video: None }).await {
            Ok(o) => o,
            Err(e) => return eprintln!("offer failed: {e:?}"),
        };
        if pc.signaling_state() != SignalingState::Stable {
            return;
        }
        if let Err(e) = pc.set_local_description(offer.clone()).await {
            return eprintln!("set local offer failed: {e:?}");
        }
        if self.peers.get(id).map(|p| p.gen) == Some(g) {
            dbg_sig!("[{id}] sent offer");
            self.relay_send(id, &json!({"type": "signal", "data": {"description": sdp_json(&offer)}}));
        }
    }

    async fn on_signal(&mut self, from: &str, data: Value) {
        if data.get("reset").and_then(|r| r.as_bool()).unwrap_or(false) {
            // They started over: drop our old connection too (unless we just made a fresh one).
            let fresh = self.peers.get(from).and_then(|p| p.rebuilt).map(|t| t.elapsed() < Duration::from_secs(3)).unwrap_or(false);
            if self.peers.contains_key(from) && !fresh {
                self.close_peer(from);
                self.sync_media();
            }
            return;
        }
        if !self.peers.contains_key(from) && !self.make_peer(from) {
            return;
        }
        let p = self.peers.get(from).unwrap();
        let (mut pc, mut gen, polite) = (p.pc.clone(), p.gen, p.polite);
        if let Some(d) = data.get("description") {
            let ty: SdpType = match d.get("type").and_then(|t| t.as_str()).and_then(|t| t.parse().ok()) {
                Some(t) => t,
                None => return,
            };
            let sdp = d.get("sdp").and_then(|s| s.as_str()).unwrap_or("");
            let Ok(desc) = SessionDescription::parse(sdp, ty) else { return };
            let stable = pc.signaling_state() == SignalingState::Stable;
            let clash = ty == SdpType::Offer && !stable;
            dbg_sig!("[{from}] got {ty} in {:?} polite={polite}", pc.signaling_state());
            if clash && !polite {
                dbg_sig!("[{from}] ignored their offer (clash)");
                return; // the impolite side ignores the other offer; they will answer ours
            }
            if ty == SdpType::Answer && pc.signaling_state() != SignalingState::HaveLocalOffer {
                return; // an old answer
            }
            if clash {
                match SessionDescription::parse("", SdpType::Rollback) {
                    Ok(rb) => {
                        let r = pc.set_local_description(rb).await;
                        dbg_sig!("[{from}] rollback: {r:?} now {:?}", pc.signaling_state());
                    }
                    Err(e) => dbg_sig!("[{from}] rollback parse failed: {e:?}"),
                }
            }
            if let Err(e) = pc.set_remote_description(desc.clone()).await {
                if ty != SdpType::Offer {
                    return eprintln!("set remote {ty} failed: {e:?}");
                }
                // libwebrtc can't always take an offer after rolling back its own (Firefox numbers
                // its RTP header extensions differently). Answer it on a fresh connection; our own
                // tracks are added again right after.
                dbg_sig!("[{from}] their offer failed ({e:?}); answering on a fresh connection");
                self.close_peer(from);
                if !self.make_peer(from) {
                    return;
                }
                let p = self.peers.get(from).unwrap();
                (pc, gen) = (p.pc.clone(), p.gen);
                if let Err(e) = pc.set_remote_description(desc).await {
                    return eprintln!("set remote offer failed again: {e:?}");
                }
            }
            if ty == SdpType::Offer {
                let ans = match pc.create_answer(AnswerOptions::default()).await {
                    Ok(a) => a,
                    Err(e) => return eprintln!("answer failed: {e:?}"),
                };
                if let Err(e) = pc.set_local_description(ans.clone()).await {
                    return eprintln!("set local answer failed: {e:?}");
                }
                if self.peers.get(from).map(|p| p.gen) == Some(gen) {
                    dbg_sig!("[{from}] sent answer");
                    self.relay_send(from, &json!({"type": "signal", "data": {"description": sdp_json(&ans)}}));
                }
            }
            if ty == SdpType::Offer && self.peers.get(from).map(|p| p.out.is_empty()).unwrap_or(false) {
                self.sync_media(); // a fresh connection: add our tracks back
            }
            self.maybe_offer(from).await;
        } else if let Some(c) = data.get("candidate") {
            let cand = c.get("candidate").and_then(|x| x.as_str()).unwrap_or("");
            if cand.is_empty() {
                return;
            }
            let mid = c.get("sdpMid").and_then(|x| x.as_str()).unwrap_or("");
            let idx = c.get("sdpMLineIndex").and_then(|x| x.as_i64()).unwrap_or(0) as i32;
            if let Ok(ic) = IceCandidate::parse(mid, idx, cand) {
                let _ = pc.add_ice_candidate(ic).await;
            }
        }
    }

    fn on_conn(&mut self, id: &str, g: u64, st: PeerConnectionState) {
        let Some(p) = self.peers.get_mut(id) else { return };
        if p.gen != g {
            return;
        }
        p.state = st;
        if st == PeerConnectionState::Failed {
            // A dead connection can't always be revived, so both sides start over with a fresh one.
            let tries = self.retries.entry(id.to_string()).or_insert(0);
            *tries += 1;
            let t = *tries;
            if t <= 4 {
                let tx = self.tx.clone();
                let pid = id.to_string();
                tokio::spawn(async move {
                    tokio::time::sleep(Duration::from_millis(500 * t as u64)).await;
                    let _ = tx.send(Ev::Rebuild(pid, g));
                });
            } else if !p.warned {
                p.warned = true;
                let name = self.room.as_ref().and_then(|r| r.people.get(id)).map(|p| p.name.clone()).unwrap_or_else(|| "someone".into());
                self.emit(CallEvent::Toast(format!("Couldn't connect to {name} in the call. Their network may be blocking it.")));
            }
        }
        if st == PeerConnectionState::Connected {
            self.retries.remove(id);
        }
    }

    fn on_track(&mut self, id: &str, g: u64, track: MediaStreamTrack, stream_id: String, has_video: bool) {
        if self.peers.get(id).map(|p| p.gen) != Some(g) {
            return;
        }
        let tid = track.id();
        if self.remote.iter().any(|r| r.track.id() == tid && r.peer == id) {
            return;
        }
        let kind = match &track {
            MediaStreamTrack::Video(_) => "video",
            MediaStreamTrack::Audio(_) if has_video => "screen-audio",
            MediaStreamTrack::Audio(_) => "voice",
        };
        let k = format!("{id}:{tid}");
        let mut task = None;
        match &track {
            MediaStreamTrack::Video(v) => video::watch_remote(&self.frames, &k, v.clone()),
            MediaStreamTrack::Audio(a) => {
                // Their audio goes into the speaker mix, 10 ms at a time.
                self.audio().add_remote(&k, 0.0);
                let shared = self.audio().shared.clone();
                let (a, key) = (a.clone(), k.clone());
                task = Some(tokio::spawn(async move {
                    use futures_util::StreamExt;
                    let mut st = libwebrtc::audio_stream::native::NativeAudioStream::new(a, super::audio::RATE as i32, 1);
                    dbg_sig!("[{key}] listening");
                    let mut n = 0u64;
                    while let Some(f) = st.next().await {
                        if n == 0 { dbg_sig!("[{key}] first audio frame"); }
                        n += 1;
                        super::audio::Audio::push(&shared, &key, &f.data);
                    }
                    dbg_sig!("[{key}] audio ended after {n} frames");
                }).abort_handle());
            }
        }
        self.remote.push(Remote { peer: id.to_string(), gen: g, kind, stream_id, track, key: Some(k), task });
        self.apply_audio();
    }

    /// Voice is heard only from people in the same channel (on a stage, only from speakers).
    fn apply_audio(&mut self) {
        let Some(r) = self.room.as_ref() else { return };
        let stage = r.is_stage();
        for rm in &self.remote {
            if matches!(rm.track, MediaStreamTrack::Video(_)) {
                continue;
            }
            let st = r.state_of(&rm.peer);
            let hear = !r.me.deaf && r.me.voice.is_some() && st.voice == r.me.voice && !(rm.kind == "voice" && stage && !st.speaker);
            let on = hear && !self.local_mute.contains(&rm.peer);
            if let (Some(a), Some(k)) = (&self.audio, &rm.key) {
                a.set_gain(k, if on { 1.0 } else { 0.0 });
            }
        }
        if let Some(a) = &self.audio {
            a.shared.deaf.store(r.me.deaf, std::sync::atomic::Ordering::Relaxed);
        }
    }

    fn on_tick(&mut self) {
        // Remote videos whose stream ended (they turned the camera off).
        let Some(r) = self.room.as_ref() else { return };
        let mut drop = Vec::new();
        for (i, rm) in self.remote.iter().enumerate() {
            if rm.kind == "voice" {
                continue;
            }
            let st = r.state_of(&rm.peer);
            let live = st.cam.as_deref() == Some(rm.stream_id.as_str()) || st.screen.as_deref() == Some(rm.stream_id.as_str());
            if !live && self.peers.get(&rm.peer).map(|p| p.gen) == Some(rm.gen) && st.voice.is_some() {
                // Keep it while their state catches up; the stream id usually arrives first.
                if !(st.cam.is_none() && st.screen.is_none()) {
                    continue;
                }
                drop.push(i);
            }
        }
        for i in drop.into_iter().rev() {
            let rm = self.remote.remove(i);
            self.drop_remote(rm);
        }
    }

    async fn collect_stats(&mut self) {
        let in_voice = self.room.as_ref().map(|r| r.me.voice.is_some()).unwrap_or(false);
        if !in_voice {
            self.speaking.clear();
            self.stats.clear();
            return;
        }
        let mut speaking = HashSet::new();
        let mut stats = Vec::new();
        let mut mic_level = 0.0f64;
        let list: Vec<(String, PeerConnection, PeerConnectionState)> = self.peers.iter().map(|(id, p)| (id.clone(), p.pc.clone(), p.state)).collect();
        for (id, pc, st) in list {
            if std::env::var_os("LC_DEBUG").is_some() {
                let trs: Vec<String> = pc.transceivers().iter().map(|t| format!("{:?}:{:?}", t.mid(), t.direction())).collect();
                eprintln!("[{id}] {:?} transceivers {}", pc.signaling_state(), trs.join(" "));
            }
            let Ok(all) = pc.get_stats().await else { continue };
            let mut ps = PeerStats { id: id.clone(), state: format!("{st:?}"), ..Default::default() };
            let mut codecs = HashMap::new();
            for s in &all {
                if let RtcStats::Codec(c) = s {
                    codecs.insert(c.rtc.id.clone(), c.codec.mime_type.clone());
                }
            }
            let mut used = Vec::new();
            for s in all {
                match s {
                    RtcStats::InboundRtp(i) if i.stream.kind == "audio" => {
                        ps.audio_in_bytes += i.inbound.bytes_received;
                        ps.audio_in_level = ps.audio_in_level.max(i.inbound.audio_level);

                        if let Some(c) = codecs.get(&i.stream.codec_id) { used.push(c.clone()); }
                    }
                    RtcStats::InboundRtp(i) if i.stream.kind == "video" => {
                        ps.video_in_frames += i.inbound.frames_decoded as u64;
                        if let Some(c) = codecs.get(&i.stream.codec_id) { used.push(c.clone()); }
                    }
                    RtcStats::OutboundRtp(o) if o.stream.kind == "audio" => ps.audio_out_bytes += o.sent.bytes_sent,
                    RtcStats::OutboundRtp(o) if o.stream.kind == "video" => {
                        ps.video_out_frames += o.outbound.frames_encoded as u64;
                        if let Some(c) = codecs.get(&o.stream.codec_id) { used.push(c.clone()); }
                    }
                    RtcStats::MediaSource(m) if m.source.kind == "audio" => mic_level = mic_level.max(m.audio.audio_level),
                    _ => {}
                }
            }
            used.sort();
            used.dedup();
            ps.codecs = used.join(",");
            stats.push(ps);
        }
        let _ = mic_level;
        // Who's speaking, from the levels the audio thread measures (like the web app's 0.03).
        if let (Some(r), Some(a)) = (self.room.as_ref(), self.audio.as_ref()) {
            if a.mic_level() > 0.02 && !r.me.muted && !r.me.deaf {
                speaking.insert(r.my_id.clone());
            }
            let levels = a.levels();
            dbg_sig!("levels {:?} frames {:?} remotes {:?}", levels, a.frame_counts(), self.remote.iter().map(|r| (r.kind, r.key.clone())).collect::<Vec<_>>());
            for rm in &self.remote {
                if rm.kind == "voice" && rm.key.as_ref().and_then(|k| levels.get(k)).is_some_and(|l| *l > 0.02) {
                    speaking.insert(rm.peer.clone());
                }
            }
        }
        if speaking != self.speaking {
            self.speaking = speaking;
            self.emit(CallEvent::Changed);
        }
        self.stats = stats;
    }

    // ---------- camera and screen ----------

    fn start_camera(&mut self) {
        if self.cam.is_some() || !self.room.as_ref().map(|r| r.can_send()).unwrap_or(false) {
            return;
        }
        self.gen += 1;
        let gen = self.gen;
        let tx = self.tx.clone();
        let my_id = self.room.as_ref().map(|r| r.my_id.clone()).unwrap_or_default();
        match video::Capture::camera(&self.frames, &format!("{my_id}:cam"), Box::new(move || { let _ = tx.send(Ev::CaptureEnded("cam", gen)); })) {
            Ok((track, cap)) => {
                let enc = RtpEncodingParameters { max_bitrate: Some(1_500_000), max_framerate: Some(30.0), ..Default::default() };
                let sid = format!("cam-{}", super::room::rid(6));
                self.cam = Some((Local { stream_id: sid.clone(), tracks: vec![Src::Video(track)], enc: Some(enc), gen }, cap));
                if let Some(r) = self.room.as_mut() {
                    r.me.cam = Some(sid);
                    r.send_state();
                }
                self.flush();
                self.sync_media();
            }
            Err(e) => self.emit(CallEvent::Toast(format!("Couldn't turn on your camera: {e}"))),
        }
    }

    fn stop_camera(&mut self) {
        if let Some((_, cap)) = self.cam.take() {
            cap.stop();
            if let Some(r) = self.room.as_mut() {
                r.me.cam = None;
                r.send_state();
            }
            self.flush();
            self.sync_media();
        }
    }

    fn start_screen(&mut self, source: Option<u64>) {
        if self.screen.is_some() || !self.room.as_ref().map(|r| r.can_send()).unwrap_or(false) {
            return;
        }
        self.gen += 1;
        let gen = self.gen;
        let tx = self.tx.clone();
        let my_id = self.room.as_ref().map(|r| r.my_id.clone()).unwrap_or_default();
        match video::Capture::screen(&self.frames, &format!("{my_id}:screen"), source, Box::new(move || { let _ = tx.send(Ev::CaptureEnded("screen", gen)); })) {
            Ok((track, cap)) => {
                let enc = RtpEncodingParameters { max_bitrate: Some(4_000_000), max_framerate: Some(30.0), ..Default::default() };
                let sid = format!("screen-{}", super::room::rid(6));
                self.screen = Some((Local { stream_id: sid.clone(), tracks: vec![Src::Video(track)], enc: Some(enc), gen }, cap));
                if let Some(r) = self.room.as_mut() {
                    r.me.screen = Some(sid);
                    r.send_state();
                }
                self.emit(CallEvent::Sound("streamStart"));
                self.flush();
                self.sync_media();
            }
            Err(e) => self.emit(CallEvent::Toast(format!("Couldn't share your screen: {e}"))),
        }
    }

    fn stop_screen(&mut self) {
        if let Some((_, cap)) = self.screen.take() {
            cap.stop();
            if let Some(r) = self.room.as_mut() {
                r.me.screen = None;
                r.send_state();
            }
            self.emit(CallEvent::Sound("streamStop"));
            self.flush();
            self.sync_media();
        }
    }

    /// Copies the state the UI shows into the shared view.
    fn publish(&mut self) {
        let mut v = CallView::default();
        if let Some(r) = self.room.as_ref() {
            v.room = Some(r.name.clone());
            v.connected = self.connected;
            v.ready = r.ready;
            v.host = r.am_host();
            v.my_id = r.my_id.clone();
            v.voice = r.me.voice.clone();
            v.muted = r.me.muted;
            v.deaf = r.me.deaf;
            v.cam = self.cam.is_some();
            v.screen = self.screen.is_some();
            v.since = self.since;
            let mut me = PersonView { id: r.my_id.clone(), uid: r.my_uid.clone(), name: r.my_name.clone(), status: r.status.clone(), state: r.my_state() };
            me.state.speaker = r.self_auth.speaker;
            v.people.push(me);
            v.people.extend(r.people.values().map(|p| PersonView { id: p.id.clone(), uid: p.uid.clone(), name: p.name.clone(), status: p.status.clone(), state: p.state.clone() }));
            v.channels = r.channels.iter().map(|c| ChannelView {
                id: c.get("id").and_then(|x| x.as_str()).unwrap_or("").into(),
                name: c.get("name").and_then(|x| x.as_str()).unwrap_or("").into(),
                kind: c.get("kind").and_then(|x| x.as_str()).unwrap_or("").into(),
            }).collect();
            for (ch, list) in &r.history {
                v.history.insert(ch.clone(), list.iter().map(|m| ChatView {
                    mid: m.get("mid").and_then(|x| x.as_str()).unwrap_or("").into(),
                    uid: m.get("uid").and_then(|x| x.as_str()).unwrap_or("").into(),
                    name: m.get("name").and_then(|x| x.as_str()).unwrap_or("").into(),
                    text: m.get("text").and_then(|x| x.as_str()).unwrap_or("").into(),
                    ts: m.get("ts").and_then(|x| x.as_f64()).unwrap_or(0.0) as i64,
                    edited: m.get("edited").map(|e| !e.is_null()).unwrap_or(false),
                    file: m.get("file").and_then(|f| f.get("name")).and_then(|x| x.as_str()).map(String::from),
                }).collect());
            }
            let now = super::room::now_ms();
            v.typing = r.typing.values().filter(|t| t.2 > now).map(|t| (t.0.clone(), t.1.clone())).collect();
            v.bad = self.peers.values().any(|p| p.state == PeerConnectionState::Failed);
        }
        if let Some(r) = self.room.as_ref() {
            for rm in &self.remote {
                if let Some(k) = &rm.key {
                    let st = r.state_of(&rm.peer);
                    v.video_kinds.insert(k.clone(), if st.screen.as_deref() == Some(rm.stream_id.as_str()) { "screen" } else { "cam" });
                }
            }
        }
        v.speaking = self.speaking.clone();
        v.mic_failed = self.mic_failed;
        v.stats = self.stats.clone();
        v.local_mute = self.local_mute.clone();
        let changed = {
            let mut cur = self.view.lock().unwrap();
            let changed = !same_view(&cur, &v);
            *cur = v;
            changed
        };
        if changed {
            self.emit(CallEvent::Changed);
        }
    }
}

/// Cheap check so the UI only redraws when something it shows changed.
fn same_view(a: &CallView, b: &CallView) -> bool {
    a.room == b.room && a.connected == b.connected && a.ready == b.ready && a.voice == b.voice && a.muted == b.muted && a.deaf == b.deaf
        && a.cam == b.cam && a.screen == b.screen && a.bad == b.bad && a.mic_failed == b.mic_failed && a.speaking == b.speaking
        && a.people.len() == b.people.len()
        && a.people.iter().zip(&b.people).all(|(x, y)| x.id == y.id && x.name == y.name && x.state == y.state && x.status == y.status)
        && a.channels.len() == b.channels.len()
        && a.history.iter().map(|(k, v)| (k.len(), v.len(), v.last().map(|m| m.text.len()))).collect::<Vec<_>>()
            == b.history.iter().map(|(k, v)| (k.len(), v.len(), v.last().map(|m| m.text.len()))).collect::<Vec<_>>()
        && a.typing == b.typing && a.video_kinds == b.video_kinds
}

/// Adds or removes one of our streams on a connection (like the web app's setStream).
fn set_stream(p: &mut Peer, key: &'static str, want: Option<(u64, String, Vec<Src>, Option<RtpEncodingParameters>)>) {
    let cur = p.out.get(key).map(|(g, _)| *g);
    if cur == want.as_ref().map(|w| w.0) {
        return;
    }
    if let Some((_, senders)) = p.out.remove(key) {
        for s in senders {
            let _ = p.pc.remove_track(s);
        }
    }
    let Some((g, stream_id, srcs, enc)) = want else { return };
    let mut tracks: Vec<MediaStreamTrack> = srcs.into_iter().map(|s| match s {
        Src::Audio(a) => MediaStreamTrack::Audio(p.factory.create_audio_track(&format!("{key}-{}", super::room::rid(4)), a)),
        Src::Video(v) => MediaStreamTrack::Video(p.factory.create_video_track(&format!("{key}-{}", super::room::rid(4)), v)),
    }).collect();
    tracks.sort_by_key(|t| if matches!(t, MediaStreamTrack::Video(_)) { 0 } else { 1 });
    let mut senders = Vec::new();
    for t in tracks {
        let video = matches!(t, MediaStreamTrack::Video(_));
        let init = RtpTransceiverInit {
            direction: RtpTransceiverDirection::SendOnly,
            stream_ids: vec![stream_id.clone()],
            send_encodings: if video { enc.clone().into_iter().collect() } else { vec![] },
        };
        match p.pc.add_transceiver(t, init) {
            Ok(tr) => senders.push(tr.sender()),
            Err(e) => eprintln!("add track failed: {e:?}"),
        }
    }
    p.out.insert(key, (g, senders));
}

//! Native calls: voice, video and screen share with the same rooms and signaling as the web
//! app, so desktop and browser users can be in the same call.
//!
//! - `room`: the room protocol (who's here, who's the host, voice states, channels, chat).
//! - `relay`: the WebSocket to the relay that passes messages between everyone in a room.
//! - `media`: WebRTC (Google's libwebrtc through LiveKit's `libwebrtc` crate). Microphone and
//!   speakers go through WebRTC's own audio device module, with its echo cancellation, noise
//!   suppression and automatic gain, like a browser.
//!
//! The engine runs on its own thread. The UI (or the headless `callbot` test binary) sends
//! `Cmd`s, reads the shared `CallView`, and gets a `CallEvent` whenever something changed.
//! Nothing here knows about GPUI.

pub mod audio;
pub mod capture;
pub mod media;
pub mod relay;
pub mod room;
pub mod share_audio;
pub mod video;

use std::sync::{Arc, Mutex};

pub use room::{VoiceState, DM_VOICE};

/// How a camera or screen share looks, like the web app's quality choices.
#[derive(Debug, Clone, PartialEq)]
pub struct Quality {
    /// Height in pixels; 0 = as captured ("Source").
    pub height: u32,
    pub fps: u32,
    /// Megabits a second; 0 = picked from the size and frame rate ("Auto").
    pub mbps: f32,
}

impl Quality {
    /// Bits a second to aim for, like the web app's `shareLimits` and `camLimits`.
    pub fn bitrate(&self, screen: bool) -> u64 {
        if self.mbps > 0.0 {
            return (self.mbps.clamp(0.1, 50.0) * 1e6) as u64;
        }
        let h = if self.height == 0 { 1440.0 } else { self.height as f64 };
        let px = h * h * 16.0 / 9.0 * self.fps.max(1) as f64;
        if screen { (px * 0.1).clamp(500e3, 10e6) as u64 } else { (px * 0.07).clamp(400e3, 6e6) as u64 }
    }
}

/// Microphone settings, like the web app's Voice & Video page. `profile` sets most of them:
/// "isolation" (AI noise suppression and automatic voice activity), "studio" (nothing at all,
/// open mic) or "custom" (the fields below).
#[derive(Debug, Clone, PartialEq)]
pub struct VoiceOpts {
    /// 0 to 2.
    pub in_vol: f32,
    pub out_vol: f32,
    pub profile: String,
    /// "vad" (voice activity) or "ptt" (push to talk).
    pub mode: String,
    pub auto_sens: bool,
    /// Manual sensitivity in dB (-80 to 0).
    pub sens: f32,
    /// "ai", "standard" or "off".
    pub ns: String,
    pub ec: bool,
    pub agc: bool,
    pub ptt_delay_ms: u32,
}

impl Default for VoiceOpts {
    fn default() -> Self {
        VoiceOpts { in_vol: 1.0, out_vol: 1.0, profile: "isolation".into(), mode: "vad".into(), auto_sens: true, sens: -50.0, ns: "ai".into(), ec: true, agc: true, ptt_delay_ms: 20 }
    }
}

impl VoiceOpts {
    /// What actually applies once the profile is taken into account (the web app's nsEff, ecEff,
    /// agcEff and inputModeEff): (noise suppression, echo cancellation, gain control, mode,
    /// automatic sensitivity, manual threshold in dB).
    pub fn effective(&self) -> (&str, bool, bool, &str, bool, f32) {
        match self.profile.as_str() {
            "studio" => ("off", false, false, "vad", false, -100.0),
            "custom" => (self.ns.as_str(), self.ec, self.agc, self.mode.as_str(), self.auto_sens, self.sens),
            _ => ("ai", true, true, "vad", true, self.sens),
        }
    }
}

/// What the UI asks the engine to do.
#[derive(Debug, Clone)]
pub enum Cmd {
    /// Connect to a room (leaving any other). `uid` is our LiteCord account id.
    Open { room: String, uid: String, name: String, status: String, custom: String, saved: Option<serde_json::Value> },
    /// Leave the room (and its call).
    Close,
    /// Join a voice channel in the open room. Waits until the room's channel list has arrived.
    JoinVoice(String),
    LeaveVoice,
    SetMuted(bool),
    SetDeaf(bool),
    Camera(bool),
    /// Start sharing the screen (`Some(source id)` picks one; `None` lets the system ask) or stop.
    Screen(Option<Option<u64>>),
    /// Turn someone's audio off just for us.
    LocalMute(String, bool),
    /// How loud each person is for us (0 to 2) and who we muted for ourselves, by uid (or by
    /// peer id for someone without one). Replaces the previous lists.
    UserAudio { vol: std::collections::HashMap<String, f32>, mute: std::collections::HashSet<String> },
    Chat { ch: String, text: String },
    Typing(String),
    Status { status: String, custom: String },
    Rename(String),
    /// Pick the microphone / speakers by name (empty = system default).
    Devices { mic: String, speaker: String },
    /// Microphone and speaker settings.
    Voice(VoiceOpts),
    /// Push-to-talk key held (true) or let go.
    Ptt(bool),
    /// Screen share quality and what it sends as sound (`share_audio::NONE`, `ALL` or
    /// "app:<name>"). Quality changes apply right away while live; sound on the next share.
    Share { quality: Quality, sound: String },
    /// Which camera (by name, empty = the first) and its quality. Restarts the camera if it's on.
    Cam { device: String, quality: Quality },
    Quit,
}

#[derive(Debug, Clone)]
pub enum CallEvent {
    Changed,
    Toast(String),
    /// A sound the web app plays at this moment ("join", "leave", "userJoin", ...).
    Sound(&'static str),
    /// A text channel got a message (for unread marks).
    Chat(String),
    /// The open room's saved copy changed: (room, snapshot) to save to disk.
    Save(String, serde_json::Value),
}

#[derive(Debug, Clone, Default)]
pub struct PersonView {
    pub id: String,
    pub uid: String,
    pub name: String,
    pub status: String,
    pub state: VoiceState,
}

#[derive(Debug, Clone, Default)]
pub struct ChannelView {
    pub id: String,
    pub name: String,
    pub kind: String,
}

#[derive(Debug, Clone, Default)]
pub struct ChatView {
    pub mid: String,
    pub uid: String,
    pub name: String,
    pub text: String,
    pub ts: i64,
    pub edited: bool,
    pub file: Option<String>,
}

/// Numbers for one connection, for the test binary and the "connection problem" mark.
#[derive(Debug, Clone, Default)]
pub struct PeerStats {
    pub id: String,
    pub state: String,
    pub audio_in_bytes: u64,
    pub audio_out_bytes: u64,
    pub audio_in_level: f64,
    pub video_in_frames: u64,
    pub video_out_frames: u64,
    pub codecs: String,
}

/// Everything the UI shows about the open room and call.
#[derive(Debug, Clone, Default)]
pub struct CallView {
    pub room: Option<String>,
    pub connected: bool,
    pub ready: bool,
    pub host: bool,
    pub my_id: String,
    pub voice: Option<String>,
    pub muted: bool,
    pub deaf: bool,
    pub cam: bool,
    pub screen: bool,
    pub since: Option<std::time::Instant>,
    pub people: Vec<PersonView>,
    pub channels: Vec<ChannelView>,
    pub history: std::collections::HashMap<String, Vec<ChatView>>,
    pub typing: Vec<(String, String)>,
    pub speaking: std::collections::HashSet<String>,
    /// Someone in the call we couldn't connect to.
    pub bad: bool,
    pub mic_failed: bool,
    pub stats: Vec<PeerStats>,
    pub local_mute: std::collections::HashSet<String>,
    /// Remote video keys ("<peer>:<track>") -> "cam" or "screen".
    pub video_kinds: std::collections::HashMap<String, &'static str>,
}

impl CallView {
    pub fn in_voice(&self, ch: &str) -> Vec<PersonView> {
        let mut v = Vec::new();
        if self.voice.as_deref() == Some(ch) {
            if let Some(me) = self.people.iter().find(|p| p.id == self.my_id) {
                v.push(me.clone());
            }
        }
        v.extend(self.people.iter().filter(|p| p.id != self.my_id && p.state.voice.as_deref() == Some(ch)).cloned());
        v
    }
}

pub struct CallHandle {
    tx: tokio::sync::mpsc::UnboundedSender<media::Ev>,
    pub view: Arc<Mutex<CallView>>,
    pub frames: video::Frames,
}

impl CallHandle {
    pub fn send(&self, cmd: Cmd) {
        let _ = self.tx.send(media::Ev::Cmd(cmd));
    }
    pub fn view(&self) -> CallView {
        self.view.lock().map(|v| v.clone()).unwrap_or_default()
    }
}

/// Starts the call engine thread. `base` is the server, e.g. `https://litecord.litecord.workers.dev`.
pub fn start(base: String, events: async_channel::Sender<CallEvent>) -> CallHandle {
    let view = Arc::new(Mutex::new(CallView::default()));
    let frames = video::Frames::default();
    let tx = media::spawn(base, events, view.clone(), frames.clone());
    CallHandle { tx, view, frames }
}

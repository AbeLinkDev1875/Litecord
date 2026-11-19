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
pub mod video;

use std::sync::{Arc, Mutex};

pub use room::{VoiceState, DM_VOICE};

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

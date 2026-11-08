//! The room protocol, the same as the web app's (`cloudflare/public/index.html`).
//!
//! Everyone in a room (a server, or a DM's call room `dm-<cid>`) connects to the relay. The one
//! online longest is the host: it keeps the channel list and chat history, and everyone else
//! sends their messages to it. The host checks them and sends events to everyone ("emit").
//! If the host leaves, the next one takes over with its own copy.
//!
//! This file is only the bookkeeping. It never touches the network: whatever it wants to send
//! goes into `out` as (peer id, JSON text), and the engine passes that to the relay.

use std::collections::{BTreeMap, HashMap, HashSet};

use serde_json::{json, Map, Value};

pub const DM_VOICE: &str = "default-lounge";
const DEFAULT_CHANNELS: [(&str, &str); 4] = [("general", "text"), ("Lounge", "voice"), ("Stage", "stage"), ("forum", "forum")];
const KINDS: [&str; 5] = ["text", "voice", "stage", "forum", "post"];
const MAX_CHANNELS: usize = 100;
const HISTORY_LIMIT: usize = 500;
const POST_LIMIT: usize = 200;
const STATUS: [&str; 4] = ["online", "idle", "dnd", "invisible"];

pub fn rid(n: usize) -> String {
    (0..n).map(|_| format!("{:02x}", rand::random::<u8>())).collect()
}
pub fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}
fn one_line(v: Option<&Value>, max: usize) -> String {
    let s = match v {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string(),
    };
    s.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(max).collect()
}
fn s(v: Option<&Value>, max: usize) -> String {
    match v {
        Some(Value::String(s)) => s.chars().take(max).collect(),
        Some(Value::Null) | None => String::new(),
        Some(other) => other.to_string().chars().take(max).collect(),
    }
}
fn num(v: Option<&Value>) -> f64 {
    match v {
        Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0),
        Some(Value::String(s)) => s.trim().parse().unwrap_or(0.0),
        Some(Value::Bool(b)) => *b as i32 as f64,
        _ => 0.0,
    }
}
fn truthy(v: Option<&Value>) -> bool {
    match v {
        None | Some(Value::Null) => false,
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_f64().map(|f| f != 0.0).unwrap_or(false),
        Some(Value::String(s)) => !s.is_empty(),
        _ => true,
    }
}

/// What someone is doing in voice. Same fields as the web app's `myState()`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VoiceState {
    pub voice: Option<String>,
    pub muted: bool,
    pub deaf: bool,
    pub cam: Option<String>,
    pub screen: Option<String>,
    pub hand: bool,
    pub speaker: bool,
}

impl VoiceState {
    pub fn to_json(&self) -> Value {
        json!({"voice": self.voice, "muted": self.muted, "deaf": self.deaf, "cam": self.cam, "screen": self.screen,
               "hand": self.hand, "speaker": self.speaker})
    }
    pub fn from_json(v: &Value) -> VoiceState {
        let id = |k: &str| match v.get(k) {
            Some(Value::String(s)) if !s.is_empty() => Some(s.chars().take(64).collect()),
            _ => None,
        };
        VoiceState {
            voice: id("voice"),
            muted: truthy(v.get("muted")),
            deaf: truthy(v.get("deaf")),
            cam: id("cam"),
            screen: id("screen"),
            hand: truthy(v.get("hand")),
            speaker: truthy(v.get("speaker")),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Person {
    pub id: String,
    pub uid: String,
    pub name: String,
    pub status: String,
    pub custom: String,
    pub pf: Value,
    pub state: VoiceState,
}

impl Person {
    fn from_json(p: &Value) -> Person {
        let status = s(p.get("status"), 16);
        Person {
            id: s(p.get("id"), 64),
            uid: s(p.get("uid"), 32),
            name: Some(s(p.get("name"), 32)).filter(|n| !n.is_empty()).unwrap_or_else(|| "Someone".into()),
            status: if STATUS.contains(&status.as_str()) { status } else { "online".into() },
            custom: s(p.get("custom"), 128),
            pf: clean_pf(p.get("pf")),
            state: p.get("state").filter(|v| v.is_object()).map(VoiceState::from_json).unwrap_or_default(),
        }
    }
    fn to_json(&self) -> Value {
        json!({"id": self.id, "uid": self.uid, "name": self.name, "status": self.status, "custom": self.custom,
               "pf": self.pf, "state": self.state.to_json()})
    }
}

fn clean_pf(p: Option<&Value>) -> Value {
    match p {
        Some(p) if p.is_object() => json!({
            "about": s(p.get("about"), 190), "pronouns": s(p.get("pronouns"), 40),
            "color": (num(p.get("color")).floor() as i64) % 360, "banner": (num(p.get("banner")).floor() as i64) % 360,
        }),
        _ => Value::Null,
    }
}

/// Things the UI or the media engine may want to react to.
#[derive(Debug, Clone, PartialEq)]
pub enum RoomEvent {
    /// Someone's voice state changed (or they joined or left).
    People,
    /// We got the channel list from the host (or became the host).
    Synced,
    Chat(String),
    /// A sound the web app plays: "userJoin", "userLeave", "streamStart", "streamStop".
    Sound(&'static str),
    /// The host says we're a speaker (stage) or not.
    Toast(String),
    /// Our voice channel went away.
    LeftVoice,
}

pub struct Room {
    pub name: String,
    pub my_id: String,
    pub my_uid: String,
    pub my_name: String,
    pub status: String,
    pub custom: String,
    pub pf: Value,

    pub leader: Option<String>,
    pub present: HashSet<String>,
    fresh_socket: bool,
    pub ready: bool,
    pub have_data: bool,

    pub people: BTreeMap<String, Person>,
    /// What the host says about us.
    pub self_auth: VoiceState,
    /// What we say about ourselves.
    pub me: VoiceState,

    pub channels: Vec<Value>,
    pub history: BTreeMap<String, Vec<Value>>,
    deleted: Vec<String>,
    deleted_set: HashSet<String>,

    outbox: Vec<Value>,
    /// Messages to send: (peer id, JSON).
    pub out: Vec<(String, String)>,
    pub events: Vec<RoomEvent>,
    pub typing: HashMap<String, (String, String, i64)>,
    /// The saved copy changed (for saving to disk).
    pub dirty: bool,
}

impl Room {
    pub fn new(name: &str, my_id: &str, my_uid: &str, my_name: &str) -> Room {
        Room {
            name: name.into(),
            my_id: my_id.into(),
            my_uid: my_uid.into(),
            my_name: my_name.into(),
            status: "online".into(),
            custom: String::new(),
            pf: Value::Null,
            leader: None,
            present: HashSet::new(),
            fresh_socket: true,
            ready: false,
            have_data: false,
            people: BTreeMap::new(),
            self_auth: VoiceState::default(),
            me: VoiceState::default(),
            channels: Vec::new(),
            history: BTreeMap::new(),
            deleted: Vec::new(),
            deleted_set: HashSet::new(),
            outbox: Vec::new(),
            out: Vec::new(),
            events: Vec::new(),
            typing: HashMap::new(),
            dirty: false,
        }
    }

    /// Loads a saved copy of a server (channels and history) from an earlier visit.
    pub fn load_saved(&mut self, saved: &Value) {
        if let Some(ch) = saved.get("channels").and_then(|c| c.as_array()) {
            if !ch.is_empty() {
                self.channels = ch.clone();
                self.have_data = true;
            }
        }
        if let Some(h) = saved.get("history").and_then(|h| h.as_object()) {
            for (k, v) in h {
                if let Some(a) = v.as_array() {
                    self.history.insert(k.clone(), a.clone());
                }
            }
        }
        for d in saved.get("deleted").and_then(|d| d.as_array()).into_iter().flatten() {
            if let Some(d) = d.as_str() {
                self.add_deleted(d);
            }
        }
    }

    pub fn am_host(&self) -> bool {
        self.leader.as_deref() == Some(self.my_id.as_str())
    }
    pub fn is_dm_room(&self) -> bool {
        self.name.starts_with("dm-")
    }

    /// The relay connection (re)opened.
    pub fn socket_opened(&mut self) {
        self.fresh_socket = true;
    }

    pub fn my_state(&self) -> VoiceState {
        self.me.clone()
    }
    pub fn state_of(&self, id: &str) -> VoiceState {
        if id == self.my_id { self.my_state() } else { self.people.get(id).map(|p| p.state.clone()).unwrap_or_default() }
    }
    pub fn same_call(&self, id: &str) -> bool {
        self.me.voice.is_some() && self.people.get(id).map(|p| p.state.voice == self.me.voice).unwrap_or(false)
    }
    pub fn ch_by_id(&self, id: &str) -> Option<&Value> {
        self.channels.iter().find(|c| c.get("id").and_then(|i| i.as_str()) == Some(id))
    }
    fn find_post(&self, id: &str) -> bool {
        self.channels.iter().any(|c| {
            c.get("posts").and_then(|p| p.as_array()).map(|p| p.iter().any(|x| x.get("id").and_then(|i| i.as_str()) == Some(id))).unwrap_or(false)
        })
    }
    pub fn chan_kind(&self, id: &str) -> Option<String> {
        if let Some(c) = self.ch_by_id(id) {
            return c.get("kind").and_then(|k| k.as_str()).map(String::from);
        }
        if self.find_post(id) { Some("post".into()) } else { None }
    }
    pub fn is_stage(&self) -> bool {
        self.me.voice.as_deref().and_then(|v| self.chan_kind(v)).as_deref() == Some("stage")
    }
    pub fn can_send(&self) -> bool {
        self.me.voice.is_some() && (!self.is_stage() || self.me.speaker)
    }

    fn to_peer(&mut self, id: &str, obj: &Value) {
        if id == self.my_id {
            return;
        }
        self.out.push((id.to_string(), obj.to_string()));
    }

    /// Send something to the host, or handle it here if we are the host.
    pub fn send(&mut self, msg: Value) {
        if !self.ready {
            if self.outbox.len() < 500 {
                self.outbox.push(msg);
            }
            return;
        }
        if self.am_host() {
            let me = self.my_id.clone();
            self.host_handle(&me, &msg);
        } else if let Some(l) = self.leader.clone() {
            self.to_peer(&l, &msg);
        }
    }
    fn flush_outbox(&mut self) {
        for m in std::mem::take(&mut self.outbox) {
            self.send(m);
        }
    }
    pub fn send_state(&mut self) {
        let st = self.my_state().to_json();
        self.send(json!({"type": "state", "state": st}));
    }

    pub fn join_voice(&mut self, ch: &str) {
        if self.me.voice.as_deref() == Some(ch) {
            return;
        }
        self.me.voice = Some(ch.to_string());
        self.me.speaker = false;
        self.me.hand = false;
        self.send_state();
    }
    pub fn leave_voice(&mut self) {
        if self.me.voice.is_none() {
            return;
        }
        self.me = VoiceState { muted: self.me.muted, deaf: self.me.deaf, ..Default::default() };
        self.send_state();
    }

    fn my_status_json(&self) -> Value {
        json!(self.status)
    }

    fn hello_host(&mut self) {
        let Some(l) = self.leader.clone() else { return };
        let saved = if self.have_data && !self.is_dm_room() { self.snapshot() } else { Value::Null };
        let hello = json!({"type": "hello", "name": self.my_name, "uid": self.my_uid, "status": self.my_status_json(),
                           "custom": self.custom, "pf": self.pf, "files": [], "saved": saved});
        self.to_peer(&l, &hello);
    }

    /// The relay's list of who's here.
    pub fn on_peers(&mut self, m: &Value) {
        self.present = m.get("peers").and_then(|p| p.as_array()).into_iter().flatten()
            .filter_map(|p| p.get("id").and_then(|i| i.as_str()).map(String::from)).collect();
        let old = self.leader.clone();
        self.leader = m.get("leader").and_then(|l| l.as_str()).map(String::from);
        if self.leader.is_some() && self.leader != old {
            self.on_leader_change();
        } else if self.fresh_socket && self.leader.is_some() && !self.am_host() {
            self.hello_host();
        }
        self.fresh_socket = false;
        if self.am_host() {
            let gone: Vec<String> = self.people.keys().filter(|id| !self.present.contains(*id)).cloned().collect();
            for id in gone {
                self.emit(json!({"type": "leave", "id": id}), Some(&id));
            }
        }
    }

    fn on_leader_change(&mut self) {
        if self.am_host() {
            return self.become_host();
        }
        self.ready = false;
        self.hello_host();
    }

    fn become_host(&mut self) {
        if !self.have_data {
            self.channels = DEFAULT_CHANNELS.iter().map(|(name, kind)| {
                let mut c = json!({"id": format!("default-{}", name.to_lowercase()), "name": name, "kind": kind});
                if *kind == "forum" {
                    c["posts"] = json!([]);
                }
                c
            }).collect();
            self.history.clear();
            self.people.clear();
            self.self_auth = VoiceState::default();
            self.have_data = true;
            self.after_sync();
        }
        self.ready = true;
        self.send_state();
        self.flush_outbox();
        self.events.push(RoomEvent::People);
    }

    fn after_sync(&mut self) {
        if let Some(v) = self.me.voice.clone() {
            if self.ch_by_id(&v).is_none() {
                self.leave_voice();
                self.events.push(RoomEvent::LeftVoice);
            }
        }
        self.events.push(RoomEvent::Synced);
    }

    /// A text message from another peer (through the relay or a data channel). `signal`
    /// messages are for the media engine and should not come here.
    pub fn on_peer_text(&mut self, from: &str, m: &Value) {
        if !m.is_object() {
            return;
        }
        if self.am_host() {
            return self.host_handle(from, m);
        }
        if self.leader.as_deref() == Some(from) {
            self.on_server(m.clone());
        }
    }

    /// Apply an event here and send it to everyone.
    fn emit(&mut self, ev: Value, exclude: Option<&str>) {
        let text = ev.to_string();
        let ids: Vec<String> = self.people.keys().filter(|id| Some(id.as_str()) != exclude).cloned().collect();
        for id in ids {
            self.out.push((id, text.clone()));
        }
        self.on_server(ev);
    }

    fn auth_of(&self, id: &str) -> VoiceState {
        if id == self.my_id { self.self_auth.clone() } else { self.people.get(id).map(|p| p.state.clone()).unwrap_or_default() }
    }

    fn host_handle(&mut self, from: &str, m: &Value) {
        let ty = m.get("type").and_then(|t| t.as_str()).unwrap_or("");
        if ty == "hello" {
            return self.host_hello(from, m);
        }
        let who = if from == self.my_id {
            (self.my_uid.clone(), self.my_name.clone())
        } else if let Some(p) = self.people.get(from) {
            (p.uid.clone(), p.name.clone())
        } else {
            return;
        };
        match ty {
            "chat" => self.host_chat(from, &who, m),
            "edit" | "del" | "react" | "pin" => self.host_msg_action(&who, m),
            "typing" => {
                let ch = s(m.get("ch"), 64);
                if matches!(self.chan_kind(&ch).as_deref(), Some("text" | "post" | "voice" | "stage")) {
                    self.emit(json!({"type": "typing", "ch": ch, "id": from, "name": who.1}), None);
                }
            }
            "status" => {
                let st = s(m.get("status"), 16);
                let st = if STATUS.contains(&st.as_str()) { st } else { "online".into() };
                self.emit(json!({"type": "status", "id": from, "status": st, "custom": one_line(m.get("custom"), 128), "pf": clean_pf(m.get("pf"))}), None);
            }
            "rename" => {
                let name = one_line(m.get("name"), 32);
                if !name.is_empty() {
                    self.emit(json!({"type": "rename", "id": from, "name": name}), None);
                }
            }
            "state" => self.host_state(from, m.get("state").unwrap_or(&Value::Null)),
            "stage" => self.host_stage(from, m),
            "getfile" => {
                // We don't keep files; tell them nobody here has it.
                let fid = s(m.get("fid"), 32);
                self.to_peer(from, &json!({"type": "nofile", "fid": fid}));
            }
            "sb" => {
                let v = self.auth_of(from).voice;
                let i = num(m.get("sound")).floor();
                if let Some(v) = v {
                    if (0.0..64.0).contains(&i) {
                        self.emit(json!({"type": "sb", "id": from, "name": who.1, "ch": v, "sound": i as i64}), None);
                    }
                }
            }
            "upload" => {
                let up = s(m.get("up"), 40);
                self.to_peer(from, &json!({"type": "upload-failed", "up": up, "reason": "the host is on the desktop app, which can't take files yet"}));
            }
            _ => {}
        }
    }

    fn host_hello(&mut self, from: &str, m: &Value) {
        if from == self.my_id {
            return;
        }
        let name = Some(one_line(m.get("name"), 32)).filter(|n| !n.is_empty()).unwrap_or_else(|| "Someone".into());
        let uid = Some(s(m.get("uid"), 32)).filter(|n| !n.is_empty()).unwrap_or_else(|| from.to_string());
        let st = s(m.get("status"), 16);
        let status = if STATUS.contains(&st.as_str()) { st } else { "online".into() };
        let custom = one_line(m.get("custom"), 128);
        let pf = clean_pf(m.get("pf"));
        if !self.people.contains_key(from) {
            self.emit(json!({"type": "join", "peer": {"id": from, "uid": uid, "name": name, "status": status, "custom": custom, "pf": pf, "state": {}}}), None);
        } else {
            if let Some(p) = self.people.get_mut(from) {
                if p.uid.is_empty() {
                    p.uid = uid;
                }
            }
            self.emit(json!({"type": "status", "id": from, "status": status, "custom": custom, "pf": pf}), None);
        }
        if self.merge_saved(m.get("saved").unwrap_or(&Value::Null)) {
            let mut snap = self.snapshot();
            snap["type"] = json!("sync");
            self.emit(snap, Some(from));
        }
        let mut peers = vec![json!({"id": self.my_id, "uid": self.my_uid, "name": self.my_name, "status": self.status,
                                    "custom": self.custom, "pf": self.pf, "state": self.self_auth.to_json()})];
        peers.extend(self.people.values().filter(|p| p.id != from).map(|p| p.to_json()));
        let mut welcome = self.snapshot();
        welcome["type"] = json!("welcome");
        welcome["peers"] = json!(peers);
        welcome["you"] = self.auth_of(from).to_json();
        self.to_peer(from, &welcome);
    }

    pub fn snapshot(&self) -> Value {
        let history: Map<String, Value> = self.history.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
        let n = self.deleted.len();
        json!({"channels": self.channels, "history": history, "deleted": self.deleted[n.saturating_sub(5000)..]})
    }

    fn add_deleted(&mut self, id: &str) -> bool {
        if self.deleted_set.insert(id.to_string()) {
            self.deleted.push(id.to_string());
            if self.deleted.len() > 6000 {
                let drop: Vec<String> = self.deleted.drain(..1000).collect();
                for d in drop {
                    self.deleted_set.remove(&d);
                }
            }
            true
        } else {
            false
        }
    }

    fn drop_deleted(&mut self) {
        let del = self.deleted_set.clone();
        let gone = |v: &Value| v.get("id").and_then(|i| i.as_str()).map(|i| del.contains(i)).unwrap_or(false);
        self.channels.retain(|c| !gone(c));
        for c in self.channels.iter_mut() {
            if let Some(p) = c.get_mut("posts").and_then(|p| p.as_array_mut()) {
                p.retain(|x| !gone(x));
            }
        }
        self.history.retain(|k, _| !del.contains(k));
        for list in self.history.values_mut() {
            list.retain(|x| !x.get("mid").and_then(|m| m.as_str()).map(|m| del.contains(m)).unwrap_or(false));
        }
        if !self.channels.iter().any(|c| c.get("kind").and_then(|k| k.as_str()) == Some("text")) {
            self.channels.insert(0, json!({"id": rid(6), "name": "general", "kind": "text"}));
        }
    }

    /// Merge someone's saved copy into ours: new channels, posts and messages are added,
    /// deleted ones stay deleted. Same rules as the web app's `mergeSaved`.
    fn merge_saved(&mut self, saved: &Value) -> bool {
        let Some(chs) = saved.get("channels").and_then(|c| c.as_array()) else { return false };
        let mut changed = false;
        if let Some(d) = saved.get("deleted").and_then(|d| d.as_array()) {
            for id in d.iter().rev().take(2000).rev() {
                if let Some(id) = id.as_str() {
                    if self.add_deleted(id) {
                        changed = true;
                    }
                }
            }
        }
        if changed {
            self.drop_deleted();
        }
        for c in chs {
            let Some(id) = c.get("id").and_then(|i| i.as_str()) else { continue };
            let kind = c.get("kind").and_then(|k| k.as_str()).unwrap_or("");
            if self.deleted_set.contains(id) || !KINDS.contains(&kind) || kind == "post" {
                continue;
            }
            let idx = match self.channels.iter().position(|x| x.get("id").and_then(|i| i.as_str()) == Some(id)) {
                Some(i) => i,
                None => {
                    if self.channels.len() >= MAX_CHANNELS {
                        continue;
                    }
                    let name = Some(one_line(c.get("name"), 32)).filter(|n| !n.is_empty()).unwrap_or_else(|| "channel".into());
                    let mut mine = json!({"id": id.chars().take(32).collect::<String>(), "name": name, "kind": kind});
                    if kind == "forum" {
                        mine["posts"] = json!([]);
                    }
                    self.channels.push(mine);
                    changed = true;
                    self.channels.len() - 1
                }
            };
            if kind != "forum" {
                continue;
            }
            let Some(posts) = c.get("posts").and_then(|p| p.as_array()) else { continue };
            let del = self.deleted_set.clone();
            let mine = &mut self.channels[idx];
            if !mine.get("posts").map(|p| p.is_array()).unwrap_or(false) {
                mine["posts"] = json!([]);
            }
            let list = mine["posts"].as_array_mut().unwrap();
            for p in posts.iter().take(POST_LIMIT) {
                let Some(pid) = p.get("id").and_then(|i| i.as_str()) else { continue };
                if del.contains(pid) {
                    continue;
                }
                match list.iter_mut().find(|x| x.get("id").and_then(|i| i.as_str()) == Some(pid)) {
                    None => {
                        list.push(json!({"id": pid.chars().take(32).collect::<String>(),
                            "title": Some(one_line(p.get("title"), 100)).filter(|t| !t.is_empty()).unwrap_or_else(|| "post".into()),
                            "author": s(p.get("author"), 32), "uid": s(p.get("uid"), 64),
                            "ts": num(p.get("ts")), "last": num(p.get("last")), "count": num(p.get("count")).max(0.0)}));
                        changed = true;
                    }
                    Some(have) => {
                        if num(p.get("last")) > num(have.get("last")) {
                            have["last"] = json!(num(p.get("last")));
                            have["count"] = json!(num(have.get("count")).max(num(p.get("count"))));
                            changed = true;
                        }
                    }
                }
            }
            list.sort_by(|a, b| num(b.get("ts")).partial_cmp(&num(a.get("ts"))).unwrap_or(std::cmp::Ordering::Equal));
            let extra: Vec<Value> = if list.len() > POST_LIMIT { list.split_off(POST_LIMIT) } else { vec![] };
            for old in extra {
                if let Some(i) = old.get("id").and_then(|i| i.as_str()) {
                    self.history.remove(i);
                    self.add_deleted(i);
                }
            }
        }
        if let Some(h) = saved.get("history").and_then(|h| h.as_object()) {
            for (ch, list) in h {
                let k = self.chan_kind(ch);
                if !matches!(k.as_deref(), Some("text" | "post")) {
                    continue;
                }
                let Some(list) = list.as_array() else { continue };
                let mut mine = self.history.get(ch).cloned().unwrap_or_default();
                let mut seen: HashSet<String> = mine.iter().filter_map(|x| x.get("mid").and_then(|m| m.as_str()).map(String::from)).collect();
                let mut added = false;
                let start = list.len().saturating_sub(HISTORY_LIMIT);
                for x in &list[start..] {
                    let Some(mid) = x.get("mid").and_then(|m| m.as_str()) else { continue };
                    if self.deleted_set.contains(mid) {
                        continue;
                    }
                    if seen.contains(mid) {
                        if let Some(have) = mine.iter_mut().find(|y| y.get("mid").and_then(|m| m.as_str()) == Some(mid)) {
                            let text = x.get("text").and_then(|t| t.as_str()).unwrap_or("");
                            if num(x.get("edited")) > num(have.get("edited")) && !text.trim().is_empty() {
                                have["text"] = json!(text.chars().take(2000).collect::<String>());
                                have["edited"] = json!(num(x.get("edited")));
                                added = true;
                            }
                        }
                        continue;
                    }
                    let mut msg = json!({"type": "chat", "ch": ch, "mid": mid.chars().take(32).collect::<String>(),
                        "id": s(x.get("id"), 64), "name": Some(s(x.get("name"), 32)).filter(|n| !n.is_empty()).unwrap_or_else(|| "Someone".into()),
                        "text": s(x.get("text"), 2000), "ts": num(x.get("ts"))});
                    if let Some(u) = x.get("uid").and_then(|u| u.as_str()) {
                        msg["uid"] = json!(u.chars().take(32).collect::<String>());
                    }
                    if num(x.get("edited")) > 0.0 {
                        msg["edited"] = json!(num(x.get("edited")));
                    }
                    if truthy(x.get("pinned")) {
                        msg["pinned"] = json!(true);
                    }
                    if let Some(r) = x.get("reply").filter(|r| r.get("mid").map(|m| m.is_string()).unwrap_or(false)) {
                        msg["reply"] = json!({"mid": s(r.get("mid"), 32), "name": s(r.get("name"), 32), "text": s(r.get("text"), 120)});
                    }
                    if let Some(r) = x.get("reactions").and_then(|r| r.as_object()) {
                        let mut out = Map::new();
                        for (em, uids) in r.iter().take(20) {
                            if em.chars().count() <= 16 {
                                if let Some(u) = uids.as_array() {
                                    out.insert(em.clone(), json!(u.iter().filter(|x| x.is_string()).take(100).collect::<Vec<_>>()));
                                }
                            }
                        }
                        if !out.is_empty() {
                            msg["reactions"] = Value::Object(out);
                        }
                    }
                    if let Some(f) = x.get("file").filter(|f| f.is_object()) {
                        msg["file"] = f.clone();
                    }
                    if msg["text"].as_str().unwrap_or("").is_empty() && msg.get("file").is_none() {
                        continue;
                    }
                    seen.insert(mid.to_string());
                    mine.push(msg);
                    added = true;
                }
                if !added {
                    continue;
                }
                mine.sort_by(|a, b| {
                    num(a.get("ts")).partial_cmp(&num(b.get("ts"))).unwrap_or(std::cmp::Ordering::Equal)
                        .then_with(|| a["mid"].as_str().cmp(&b["mid"].as_str()))
                });
                let n = mine.len();
                mine.drain(..n.saturating_sub(HISTORY_LIMIT));
                self.history.insert(ch.clone(), mine);
                changed = true;
            }
        }
        changed
    }

    fn find_msg(&self, ch: &str, mid: &str) -> Option<&Value> {
        self.history.get(ch)?.iter().find(|x| x.get("mid").and_then(|m| m.as_str()) == Some(mid))
    }

    fn host_chat(&mut self, from: &str, who: &(String, String), m: &Value) {
        let ch = s(m.get("ch"), 64);
        if !matches!(self.chan_kind(&ch).as_deref(), Some("text" | "post" | "voice" | "stage")) {
            return;
        }
        let text: String = s(m.get("text"), 4000).trim().chars().take(2000).collect();
        if text.is_empty() {
            return;
        }
        let mut msg = json!({"type": "chat", "ch": ch, "mid": rid(6), "id": from, "uid": who.0, "name": who.1, "text": text, "ts": now_ms()});
        if let Some(r) = m.get("reply").and_then(|r| r.as_str()).and_then(|r| self.find_msg(&ch, r)) {
            msg["reply"] = json!({"mid": r["mid"], "name": r["name"], "text": s(r.get("text"), 120)});
        }
        self.emit(msg, None);
    }

    fn host_msg_action(&mut self, who: &(String, String), m: &Value) {
        let ch = s(m.get("ch"), 64);
        let mid = s(m.get("mid"), 32);
        let Some(t) = self.find_msg(&ch, &mid).cloned() else { return };
        let own = !who.0.is_empty() && t.get("uid").and_then(|u| u.as_str()) == Some(who.0.as_str());
        match m.get("type").and_then(|t| t.as_str()).unwrap_or("") {
            "edit" => {
                let text: String = s(m.get("text"), 4000).trim().chars().take(2000).collect();
                if own && !text.is_empty() && Some(text.as_str()) != t.get("text").and_then(|x| x.as_str()) {
                    self.emit(json!({"type": "edit", "ch": ch, "mid": mid, "text": text, "edited": now_ms()}), None);
                }
            }
            "del" => {
                if own {
                    self.emit(json!({"type": "del", "ch": ch, "mid": mid}), None);
                }
            }
            "react" => {
                let emoji = s(m.get("emoji"), 32);
                if emoji.is_empty() || emoji.chars().count() > 16 || emoji.chars().any(|c| c.is_whitespace()) {
                    return;
                }
                let arr: Vec<String> = t.get("reactions").and_then(|r| r.get(&emoji)).and_then(|a| a.as_array())
                    .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
                let on = !arr.contains(&who.0);
                let count = t.get("reactions").and_then(|r| r.as_object()).map(|r| r.len()).unwrap_or(0);
                if on && arr.is_empty() && count >= 20 {
                    return;
                }
                self.emit(json!({"type": "react", "ch": ch, "mid": mid, "emoji": emoji, "uid": who.0, "on": on}), None);
            }
            "pin" => {
                let on = !truthy(t.get("pinned"));
                self.emit(json!({"type": "pin", "ch": ch, "mid": mid, "on": on}), None);
            }
            _ => {}
        }
    }

    fn host_state(&mut self, id: &str, st: &Value) {
        if !st.is_object() {
            return;
        }
        let prev = self.auth_of(id);
        let want = st.get("voice").and_then(|v| v.as_str()).unwrap_or("");
        let k = self.chan_kind(want);
        let voice = if matches!(k.as_deref(), Some("voice" | "stage")) { Some(want.to_string()) } else { None };
        let sid = |key: &str| -> Option<String> {
            if voice.is_none() {
                return None;
            }
            match st.get(key) {
                Some(Value::String(x)) if !x.is_empty() => Some(x.chars().take(64).collect()),
                _ => None,
            }
        };
        let mut nw = VoiceState {
            voice: voice.clone(),
            muted: truthy(st.get("muted")),
            deaf: truthy(st.get("deaf")),
            cam: sid("cam"),
            screen: sid("screen"),
            hand: false,
            speaker: false,
        };
        if k.as_deref() == Some("stage") {
            let mut all: Vec<(String, VoiceState)> = vec![(self.my_id.clone(), self.self_auth.clone())];
            all.extend(self.people.values().map(|p| (p.id.clone(), p.state.clone())));
            let others = all.iter().any(|(oid, os)| oid != id && os.voice == voice && os.speaker);
            nw.speaker = if prev.voice == voice { prev.speaker } else { !others };
            let hand = truthy(st.get("hand"));
            if hand && !nw.speaker && !others {
                nw.speaker = true;
            }
            nw.hand = hand && !nw.speaker;
            if !nw.speaker {
                nw.cam = None;
                nw.screen = None;
            }
        }
        self.emit(json!({"type": "state", "id": id, "state": nw.to_json()}), None);
    }

    fn host_stage(&mut self, from: &str, m: &Value) {
        // Only a speaker can bring someone up or send them back to the audience.
        let me = self.auth_of(from);
        let Some(voice) = me.voice.clone() else { return };
        if self.chan_kind(&voice).as_deref() != Some("stage") || !me.speaker {
            return;
        }
        let target = s(m.get("id"), 64);
        let mut st = self.auth_of(&target);
        if st.voice.as_deref() != Some(voice.as_str()) {
            return;
        }
        st.speaker = truthy(m.get("speaker"));
        st.hand = false;
        if !st.speaker {
            st.cam = None;
            st.screen = None;
        }
        self.emit(json!({"type": "state", "id": target, "state": st.to_json()}), None);
    }

    /// An event from the host (or one we emitted as the host).
    fn on_server(&mut self, m: Value) {
        let ty = m.get("type").and_then(|t| t.as_str()).unwrap_or("").to_string();
        if matches!(ty.as_str(), "welcome" | "sync" | "channels" | "chat" | "post" | "edit" | "del" | "react" | "pin") {
            self.dirty = true;
        }
        match ty.as_str() {
            "welcome" => {
                self.people.clear();
                for p in m.get("peers").and_then(|p| p.as_array()).into_iter().flatten() {
                    let pid = s(p.get("id"), 64);
                    if !pid.is_empty() && pid != self.my_id {
                        self.people.insert(pid, Person::from_json(p));
                    }
                }
                self.apply_snapshot(&m);
                self.self_auth = m.get("you").filter(|v| v.is_object()).map(VoiceState::from_json).unwrap_or_default();
                self.me.speaker = self.self_auth.speaker;
                self.me.hand = self.self_auth.hand;
                self.have_data = true;
                self.ready = true;
                self.send_state();
                self.after_sync();
                self.flush_outbox();
                self.events.push(RoomEvent::People);
            }
            "sync" => {
                self.apply_snapshot(&m);
                self.after_sync();
            }
            "channels" => {
                if let Some(list) = m.get("channels").and_then(|c| c.as_array()) {
                    let keep: HashSet<String> = list.iter().flat_map(|c| {
                        let mut ids = vec![s(c.get("id"), 64)];
                        for p in c.get("posts").and_then(|p| p.as_array()).into_iter().flatten() {
                            ids.push(s(p.get("id"), 64));
                        }
                        ids
                    }).collect();
                    let old: Vec<String> = self.channels.iter().flat_map(|c| {
                        let mut ids = vec![s(c.get("id"), 64)];
                        for p in c.get("posts").and_then(|p| p.as_array()).into_iter().flatten() {
                            ids.push(s(p.get("id"), 64));
                        }
                        ids
                    }).collect();
                    for id in old {
                        if !keep.contains(&id) {
                            self.add_deleted(&id);
                        }
                    }
                    self.history.retain(|k, _| keep.contains(k));
                    self.channels = list.clone();
                    self.after_sync();
                }
            }
            "join" => {
                if let Some(p) = m.get("peer") {
                    let pid = s(p.get("id"), 64);
                    if !pid.is_empty() && pid != self.my_id {
                        self.people.insert(pid, Person::from_json(p));
                        self.events.push(RoomEvent::People);
                    }
                }
            }
            "leave" => {
                let id = s(m.get("id"), 64);
                if self.me.voice.is_some() && self.people.get(&id).map(|p| p.state.voice == self.me.voice).unwrap_or(false) {
                    self.events.push(RoomEvent::Sound("userLeave"));
                }
                self.people.remove(&id);
                self.events.push(RoomEvent::People);
            }
            "status" => {
                let id = s(m.get("id"), 64);
                if let Some(p) = self.people.get_mut(&id) {
                    let st = s(m.get("status"), 16);
                    p.status = if STATUS.contains(&st.as_str()) { st } else { "online".into() };
                    p.custom = s(m.get("custom"), 128);
                    p.pf = clean_pf(m.get("pf"));
                    self.events.push(RoomEvent::People);
                }
            }
            "rename" => {
                let id = s(m.get("id"), 64);
                if let Some(p) = self.people.get_mut(&id) {
                    let n = s(m.get("name"), 32);
                    if !n.is_empty() {
                        p.name = n;
                    }
                    self.events.push(RoomEvent::People);
                }
            }
            "state" => {
                let id = s(m.get("id"), 64);
                let st = m.get("state").filter(|v| v.is_object()).map(VoiceState::from_json).unwrap_or_default();
                self.on_state(&id, st);
            }
            "chat" => {
                let ch = s(m.get("ch"), 64);
                let list = self.history.entry(ch.clone()).or_default();
                if !list.iter().any(|x| x.get("mid") == m.get("mid")) {
                    list.push(m.clone());
                    let n = list.len();
                    list.drain(..n.saturating_sub(HISTORY_LIMIT));
                }
                self.typing.retain(|_, t| !(t.0 == ch && Some(t.1.as_str()) == m.get("name").and_then(|n| n.as_str())));
                self.events.push(RoomEvent::Chat(ch));
            }
            "post" => {
                let ch = s(m.get("ch"), 64);
                if let (Some(post), Some(c)) = (m.get("post"), self.channels.iter_mut().find(|c| c.get("id").and_then(|i| i.as_str()) == Some(ch.as_str()))) {
                    if !c.get("posts").map(|p| p.is_array()).unwrap_or(false) {
                        c["posts"] = json!([]);
                    }
                    c["posts"].as_array_mut().unwrap().insert(0, post.clone());
                }
                if let Some(first) = m.get("first").filter(|f| f.is_object()) {
                    let pid = s(first.get("ch"), 64);
                    self.history.entry(pid).or_default().push(first.clone());
                }
                self.events.push(RoomEvent::Synced);
            }
            "edit" | "del" | "react" | "pin" => {
                let ch = s(m.get("ch"), 64);
                let mid = s(m.get("mid"), 32);
                if ty == "del" {
                    self.add_deleted(&mid);
                }
                if let Some(list) = self.history.get_mut(&ch) {
                    if ty == "del" {
                        list.retain(|x| x.get("mid").and_then(|v| v.as_str()) != Some(mid.as_str()));
                    } else if let Some(t) = list.iter_mut().find(|x| x.get("mid").and_then(|v| v.as_str()) == Some(mid.as_str())) {
                        match ty.as_str() {
                            "edit" => {
                                t["text"] = m["text"].clone();
                                t["edited"] = m["edited"].clone();
                            }
                            "pin" => t["pinned"] = json!(truthy(m.get("on"))),
                            _ => {
                                let emoji = s(m.get("emoji"), 32);
                                let uid = s(m.get("uid"), 32);
                                if !t.get("reactions").map(|r| r.is_object()).unwrap_or(false) {
                                    t["reactions"] = json!({});
                                }
                                let r = t["reactions"].as_object_mut().unwrap();
                                let arr = r.entry(emoji.clone()).or_insert_with(|| json!([]));
                                let mut v: Vec<Value> = arr.as_array().cloned().unwrap_or_default();
                                v.retain(|x| x.as_str() != Some(uid.as_str()));
                                if truthy(m.get("on")) {
                                    v.push(json!(uid));
                                }
                                if v.is_empty() {
                                    r.remove(&emoji);
                                } else {
                                    *arr = json!(v);
                                }
                            }
                        }
                    }
                }
                self.events.push(RoomEvent::Chat(ch));
            }
            "typing" => {
                let id = s(m.get("id"), 64);
                if id != self.my_id {
                    self.typing.insert(id, (s(m.get("ch"), 64), s(m.get("name"), 32), now_ms() + 5000));
                    self.events.push(RoomEvent::Chat(s(m.get("ch"), 64)));
                }
            }
            _ => {}
        }
    }

    fn apply_snapshot(&mut self, m: &Value) {
        self.channels = m.get("channels").and_then(|c| c.as_array()).cloned().unwrap_or_default();
        self.deleted.clear();
        self.deleted_set.clear();
        for d in m.get("deleted").and_then(|d| d.as_array()).into_iter().flatten() {
            if let Some(d) = d.as_str() {
                self.add_deleted(d);
            }
        }
        self.history.clear();
        if let Some(h) = m.get("history").and_then(|h| h.as_object()) {
            for (ch, list) in h {
                if let Some(a) = list.as_array() {
                    self.history.insert(ch.clone(), a.clone());
                }
            }
        }
    }

    fn on_state(&mut self, id: &str, st: VoiceState) {
        if id == self.my_id {
            let was = self.me.speaker;
            self.self_auth = st.clone();
            self.me.speaker = st.speaker;
            self.me.hand = st.hand;
            if was != self.me.speaker && self.me.voice.is_some() && st.voice == self.me.voice {
                self.events.push(RoomEvent::Toast(if self.me.speaker { "You're a speaker now.".into() } else { "You're in the audience.".into() }));
            }
            self.events.push(RoomEvent::People);
            return;
        }
        let Some(p) = self.people.get_mut(id) else { return };
        let prev = std::mem::replace(&mut p.state, st.clone());
        if let Some(v) = self.me.voice.clone() {
            let v = Some(v);
            if st.voice == v && prev.voice != v {
                self.events.push(RoomEvent::Sound("userJoin"));
            } else if prev.voice == v && st.voice != v {
                self.events.push(RoomEvent::Sound("userLeave"));
            } else if st.voice == v && st.screen.is_some() != prev.screen.is_some() {
                self.events.push(RoomEvent::Sound(if st.screen.is_some() { "streamStart" } else { "streamStop" }));
            }
        }
        self.events.push(RoomEvent::People);
    }

    /// Send a chat message to a channel of this room.
    pub fn chat(&mut self, ch: &str, text: &str) {
        self.send(json!({"type": "chat", "ch": ch, "text": text}));
    }

    /// People in a voice channel, us included, in a stable order.
    pub fn in_voice(&self, ch: &str) -> Vec<(String, String, String, VoiceState)> {
        let mut v = Vec::new();
        if self.me.voice.as_deref() == Some(ch) {
            v.push((self.my_id.clone(), self.my_uid.clone(), self.my_name.clone(), self.my_state()));
        }
        for p in self.people.values() {
            if p.state.voice.as_deref() == Some(ch) {
                v.push((p.id.clone(), p.uid.clone(), p.name.clone(), p.state.clone()));
            }
        }
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn peers(ids: &[&str]) -> Value {
        json!({"type": "peers", "peers": ids.iter().map(|i| json!({"id": i, "name": i})).collect::<Vec<_>>(), "leader": ids[0]})
    }

    #[test]
    fn first_one_hosts_with_default_channels() {
        let mut r = Room::new("dm-abc", "aaaa1", "0123456789abcdef", "Me");
        r.on_peers(&peers(&["aaaa1"]));
        assert!(r.am_host() && r.ready);
        assert_eq!(r.chan_kind(DM_VOICE).as_deref(), Some("voice"));
        r.join_voice(DM_VOICE);
        assert_eq!(r.self_auth.voice.as_deref(), Some(DM_VOICE));
    }

    #[test]
    fn host_welcomes_a_guest_and_relays_state() {
        let mut h = Room::new("dm-abc", "aaaa1", "u1", "Host");
        h.on_peers(&peers(&["aaaa1"]));
        h.on_peers(&peers(&["aaaa1", "bbbb2"]));
        h.out.clear();
        h.on_peer_text("bbbb2", &json!({"type": "hello", "name": "Guest", "uid": "u2"}));
        let welcome = h.out.iter().find(|(to, t)| to == "bbbb2" && t.contains("\"welcome\"")).expect("welcome");
        let mut g = Room::new("dm-abc", "bbbb2", "u2", "Guest");
        g.on_peers(&peers(&["aaaa1", "bbbb2"]));
        assert!(g.out.iter().any(|(to, t)| to == "aaaa1" && t.contains("\"hello\"")));
        g.out.clear();
        g.on_peer_text("aaaa1", &serde_json::from_str(&welcome.1).unwrap());
        assert!(g.ready && g.ch_by_id(DM_VOICE).is_some());
        g.join_voice(DM_VOICE);
        let st = g.out.iter().rev().find(|(_, t)| t.contains("\"state\"")).unwrap().1.clone();
        h.on_peer_text("bbbb2", &serde_json::from_str(&st).unwrap());
        assert_eq!(h.people["bbbb2"].state.voice.as_deref(), Some(DM_VOICE));
        // The host tells everyone, including the guest.
        assert!(h.out.iter().any(|(to, t)| to == "bbbb2" && t.contains(DM_VOICE)));
    }

    #[test]
    fn merge_keeps_both_sides() {
        let mut h = Room::new("srv", "aaaa1", "u1", "Host");
        h.on_peers(&peers(&["aaaa1"]));
        let saved = json!({"channels": [{"id": "default-general", "name": "general", "kind": "text"}, {"id": "x1", "name": "memes", "kind": "text"}],
                           "history": {"x1": [{"mid": "m1", "id": "p", "name": "A", "text": "hi", "ts": 5}]}, "deleted": []});
        assert!(h.merge_saved(&saved));
        assert!(h.ch_by_id("x1").is_some() && h.ch_by_id("default-lounge").is_some());
        assert_eq!(h.history["x1"].len(), 1);
        assert!(!h.merge_saved(&saved));
    }
}

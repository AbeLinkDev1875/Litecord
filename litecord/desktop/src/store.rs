//! Everything LiteCord keeps on this computer, and the mailbox protocol.
//!
//! This follows the web app (cloudflare/public/index.html, "friends, direct messages and group
//! chats") so the two can talk to each other: the same mail types ("freq", "facc", "fdel", "dm",
//! "op", "typing", "gmeta"), the same DM keys ("u:<uid>" for 1:1, "g:<24 hex>" for groups) and the
//! same limits. Chats are only stored here, in a JSON file, like the web app keeps them in the browser.

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub const DM_HISTORY: usize = 2000;
pub const GROUP_MAX: usize = 10;

pub fn now_ms() -> i64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// Random hex, `n` bytes long (so 2n characters), like the web app's rid().
pub fn rid(n: usize) -> String {
    (0..n).map(|_| format!("{:02x}", rand::random::<u8>())).collect()
}

pub fn is_uid(s: &str) -> bool {
    s.len() == 16 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub fn is_gid(s: &str) -> bool {
    s.len() == 24 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn is_mid(s: &str) -> bool {
    s.len() == 12 && s.bytes().all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// One line, trimmed and cut to `max` characters (names, status text).
pub fn one_line(v: Option<&Value>, max: usize) -> String {
    let s = v.and_then(|v| v.as_str()).unwrap_or("");
    s.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(max).collect()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FriendState {
    Friend,
    In,
    Out,
    Blocked,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Friend {
    pub name: String,
    pub state: FriendState,
    #[serde(default)]
    pub since: i64,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Dm {
    pub key: String,
    #[serde(default)]
    pub group: bool,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub members: BTreeMap<String, String>,
    #[serde(default)]
    pub owner: String,
    #[serde(default)]
    pub cid: String,
    #[serde(default)]
    pub last: i64,
    #[serde(default)]
    pub closed: bool,
    #[serde(default)]
    pub left: bool,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Msg {
    pub mid: String,
    #[serde(default)]
    pub uid: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub text: String,
    pub ts: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub edited: Option<i64>,
    #[serde(default)]
    pub sys: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Presence {
    #[serde(default)]
    pub status: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub custom: String,
    #[serde(default)]
    pub act: String,
    #[serde(default)]
    pub live: bool,
}

/// What's saved to disk.
#[derive(Default, Serialize, Deserialize)]
pub struct Saved {
    pub uid: String,
    pub key: String,
    pub name: String,
    #[serde(default)]
    pub friends: BTreeMap<String, Friend>,
    #[serde(default)]
    pub dms: BTreeMap<String, Dm>,
    #[serde(default)]
    pub msgs: HashMap<String, Vec<Msg>>,
    /// Your claimed username (friends can add you by it), if any.
    #[serde(default)]
    pub username: String,
    /// The DM that was open last time ("" for the Friends page), so it reopens like Discord.
    #[serde(default)]
    pub last_view: String,
}

/// Things the UI should show once (a toast at the bottom).
pub enum Notice {
    Info(String),
}

pub struct Store {
    pub me: Saved,
    pub presence: HashMap<String, Option<Presence>>,
    pub typing: HashMap<String, (String, String, i64)>, // from uid -> (dm key, name, until)
    pub unread: HashMap<String, usize>,
    pub online: bool,
    pub notices: Vec<Notice>,
    /// JSON the network should send next.
    pub outbox: Vec<String>,
    seen_mail: std::collections::HashSet<i64>,
    path: PathBuf,
    /// "online", "idle", "dnd" or "invisible", and the custom status text (from Prefs).
    pub status: String,
    pub custom: String,
}

pub fn data_dir() -> PathBuf {
    // LITECORD_DATA picks another folder (handy for running two copies side by side).
    if let Some(d) = std::env::var_os("LITECORD_DATA") {
        return PathBuf::from(d);
    }
    dirs::data_dir().unwrap_or_else(|| PathBuf::from(".")).join("litecord-desktop")
}

impl Store {
    pub fn load() -> Option<Store> {
        let path = data_dir().join("state.json");
        let me: Saved = serde_json::from_str(&std::fs::read_to_string(&path).ok()?).ok()?;
        if !is_uid(&me.uid) || me.key.len() < 16 || me.name.is_empty() {
            return None;
        }
        Some(Store::from_saved(me, path))
    }

    fn from_saved(me: Saved, path: PathBuf) -> Store {
        Store {
            me,
            presence: HashMap::new(),
            typing: HashMap::new(),
            unread: HashMap::new(),
            online: false,
            notices: Vec::new(),
            outbox: Vec::new(),
            seen_mail: Default::default(),
            path,
            status: "online".into(),
            custom: String::new(),
        }
    }

    /// A brand-new identity on this computer.
    pub fn create(name: &str) -> Store {
        let me = Saved { uid: rid(8), key: rid(24), name: name.to_string(), ..Default::default() };
        let s = Store::from_saved(me, data_dir().join("state.json"));
        s.save();
        s
    }

    /// Takes over the identity from the web app's "Link the desktop app" code.
    pub fn link(code: &str, fallback_name: &str) -> Result<Store, String> {
        let raw = code.trim().strip_prefix("lc1:").ok_or("That isn't a LiteCord link code. It starts with lc1:")?;
        let bytes = base64::engine::general_purpose::STANDARD.decode(raw.trim()).map_err(|_| "The link code is incomplete. Copy it again.")?;
        let v: Value = serde_json::from_slice(&bytes).map_err(|_| "The link code is damaged. Copy it again.")?;
        let uid = v["uid"].as_str().unwrap_or_default().to_string();
        let key = v["key"].as_str().unwrap_or_default().to_string();
        if !is_uid(&uid) || key.len() < 16 {
            return Err("The link code is missing your account. Copy it again.".into());
        }
        let mut name = one_line(v.get("name"), 32);
        if name.is_empty() {
            name = fallback_name.to_string();
        }
        let friends: BTreeMap<String, Friend> = serde_json::from_value(v["friends"].clone()).unwrap_or_default();
        let mut dms: BTreeMap<String, Dm> = BTreeMap::new();
        if let Some(obj) = v["dms"].as_object() {
            for (k, d) in obj {
                if let Ok(mut d) = serde_json::from_value::<Dm>(d.clone()) {
                    d.key = k.clone();
                    dms.insert(k.clone(), d);
                }
            }
        }
        let username = v["username"].as_str().unwrap_or_default().to_string();
        let me = Saved { uid, key, name, username, friends: friends.into_iter().filter(|(u, _)| is_uid(u)).collect(), dms, ..Default::default() };
        let s = Store::from_saved(me, data_dir().join("state.json"));
        s.save();
        Ok(s)
    }

    pub fn save(&self) {
        let _ = std::fs::create_dir_all(self.path.parent().unwrap());
        let tmp = self.path.with_extension("tmp");
        if let Ok(s) = serde_json::to_string(&self.me) {
            if std::fs::write(&tmp, s).is_ok() {
                let _ = std::fs::rename(&tmp, &self.path);
            }
        }
    }

    pub fn inbox_url(&self, base: &str) -> String {
        let ws = base.replacen("https://", "wss://", 1).replacen("http://", "ws://", 1);
        format!("{ws}/inbox?uid={}&key={}", self.me.uid, self.me.key)
    }

    /* ---------- people ---------- */

    pub fn friend_uids(&self) -> Vec<String> {
        self.me.friends.iter().filter(|(_, f)| f.state == FriendState::Friend).map(|(u, _)| u.clone()).collect()
    }

    pub fn name_of(&self, uid: &str) -> String {
        if uid == self.me.uid {
            return self.me.name.clone();
        }
        if let Some(Some(p)) = self.presence.get(uid) {
            if !p.name.is_empty() {
                return p.name.clone();
            }
        }
        if let Some(f) = self.me.friends.get(uid) {
            return f.name.clone();
        }
        for d in self.me.dms.values() {
            if let Some(n) = d.members.get(uid) {
                return n.clone();
            }
        }
        "Someone".into()
    }

    /// "online", "idle", "dnd" or "offline".
    pub fn status_of(&self, uid: &str) -> &str {
        match self.presence.get(uid) {
            Some(Some(p)) => p.status.as_str(),
            _ => "offline",
        }
    }

    pub fn subline(&self, uid: &str) -> String {
        match self.presence.get(uid) {
            Some(Some(p)) if p.live => "Sharing their screen".into(),
            Some(Some(p)) if !p.custom.is_empty() => p.custom.clone(),
            Some(Some(p)) => p.act.clone(),
            _ => String::new(),
        }
    }

    /// What friends see. Invisible means nothing at all, so you look offline.
    fn my_presence(&self) -> Value {
        if self.status == "invisible" {
            return Value::Null;
        }
        json!({"status": self.status, "name": self.me.name, "custom": self.custom, "act": "", "live": false,
               "pf": {"about": "", "pronouns": "", "color": -1, "banner": -1}})
    }

    pub fn set_status(&mut self, status: &str, custom: &str) {
        self.status = status.to_string();
        self.custom = custom.chars().take(128).collect();
        self.outbox.push(json!({"type": "presence", "presence": self.my_presence()}).to_string());
    }

    pub fn rename(&mut self, name: &str) {
        let name: String = name.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(32).collect();
        if name.is_empty() || name == self.me.name {
            return;
        }
        self.me.name = name;
        self.outbox.push(json!({"type": "presence", "presence": self.my_presence()}).to_string());
        self.save();
    }

    pub fn hello(&self) -> String {
        json!({"type": "hello", "friends": self.friend_uids(), "presence": self.my_presence()}).to_string()
    }

    fn friends_changed(&mut self) {
        self.outbox.push(json!({"type": "friends", "friends": self.friend_uids(), "presence": self.my_presence()}).to_string());
        self.save();
    }

    fn mail(&mut self, to: &[String], body: Value, eph: bool) {
        let to: Vec<&String> = to
            .iter()
            .filter(|u| is_uid(u) && **u != self.me.uid && self.me.friends.get(*u).map(|f| f.state) != Some(FriendState::Blocked))
            .collect();
        for chunk in to.chunks(12) {
            self.outbox.push(json!({"type": "send", "to": chunk, "body": body, "eph": eph}).to_string());
        }
    }

    pub fn request_friend(&mut self, uid: &str, name: Option<String>) -> Result<String, String> {
        if uid == self.me.uid {
            return Err("That's you! You can't add yourself.".into());
        }
        match self.me.friends.get(uid).map(|f| f.state) {
            Some(FriendState::Friend) => return Err("You're already friends with them!".into()),
            Some(FriendState::In) => {
                self.accept_friend(uid);
                return Ok(format!("You and {} are now friends!", self.name_of(uid)));
            }
            _ => {}
        }
        let name = name.unwrap_or_else(|| self.name_of(uid));
        self.me.friends.insert(uid.into(), Friend { name: name.clone(), state: FriendState::Out, since: now_ms() });
        let me = self.me.name.clone();
        self.mail(&[uid.into()], json!({"t": "freq", "n": me}), false);
        self.save();
        Ok(if name == "Someone" { "Success! Your friend request was sent.".into() } else { format!("Success! Your friend request to {name} was sent.") })
    }

    pub fn accept_friend(&mut self, uid: &str) {
        let Some(f) = self.me.friends.get_mut(uid) else { return };
        f.state = FriendState::Friend;
        f.since = now_ms();
        let me = self.me.name.clone();
        self.mail(&[uid.into()], json!({"t": "facc", "n": me}), false);
        self.friends_changed();
    }

    pub fn remove_friend(&mut self, uid: &str) {
        if self.me.friends.remove(uid).is_none() {
            return;
        }
        self.presence.remove(uid);
        self.mail(&[uid.into()], json!({"t": "fdel"}), false);
        self.friends_changed();
    }

    /* ---------- DMs ---------- */

    pub fn dm_for(&mut self, uid: &str) -> String {
        let key = format!("u:{uid}");
        let name = self.name_of(uid);
        let d = self.me.dms.entry(key.clone()).or_insert_with(|| Dm {
            key: key.clone(),
            members: BTreeMap::from([(uid.to_string(), name)]),
            cid: rid(12),
            last: now_ms(),
            ..Default::default()
        });
        d.closed = false;
        self.save();
        key
    }

    pub fn dm_targets(&self, key: &str) -> Vec<String> {
        self.me.dms.get(key).map(|d| d.members.keys().filter(|u| **u != self.me.uid).cloned().collect()).unwrap_or_default()
    }

    pub fn dm_title(&self, key: &str) -> String {
        let Some(d) = self.me.dms.get(key) else { return "Direct Message".into() };
        if !d.group {
            return d.members.keys().next().map(|u| self.name_of(u)).unwrap_or_default();
        }
        if !d.name.is_empty() {
            return d.name.clone();
        }
        let names: Vec<String> = d.members.iter().filter(|(u, _)| **u != self.me.uid).map(|(_, n)| n.clone()).collect();
        if names.is_empty() { "Unnamed Group".into() } else { names.join(", ") }
    }

    /// The other person in a 1:1 DM.
    pub fn dm_other(&self, key: &str) -> Option<String> {
        self.me.dms.get(key).filter(|d| !d.group).and_then(|d| d.members.keys().next().cloned())
    }

    /// DMs for the sidebar, newest first.
    pub fn dm_list(&self) -> Vec<String> {
        let mut v: Vec<&Dm> = self.me.dms.values().filter(|d| !d.closed).collect();
        v.sort_by(|a, b| b.last.cmp(&a.last));
        v.into_iter().map(|d| d.key.clone()).collect()
    }

    fn key_part(key: &str) -> Value {
        match key.strip_prefix("g:") {
            Some(k) => json!(k),
            None => Value::Null,
        }
    }

    fn group_out(d: &Dm) -> Value {
        json!({"name": d.name, "members": d.members, "owner": d.owner})
    }

    pub fn send_text(&mut self, key: &str, text: &str) {
        let text: String = text.trim().chars().take(2000).collect();
        if text.is_empty() {
            return;
        }
        let Some(d) = self.me.dms.get(key).cloned() else { return };
        if d.left {
            self.notices.push(Notice::Info("You're not in this group anymore.".into()));
            return;
        }
        let msg = json!({"mid": rid(6), "text": text, "ts": now_ms()});
        let mut body = json!({"t": "dm", "n": self.me.name, "msg": msg});
        if d.group {
            body["k"] = Self::key_part(key);
            body["g"] = Self::group_out(&d);
        } else {
            body["cid"] = json!(d.cid);
        }
        let to = self.dm_targets(key);
        self.mail(&to, body, false);
        let m = Msg { mid: msg["mid"].as_str().unwrap().into(), uid: self.me.uid.clone(), name: self.me.name.clone(), text, ts: now_ms(), ..Default::default() };
        self.add_msg(key, m);
    }

    pub fn send_typing(&mut self, key: &str) {
        let mut body = json!({"t": "typing", "n": self.me.name});
        if key.starts_with("g:") {
            body["k"] = Self::key_part(key);
        }
        let to = self.dm_targets(key);
        self.mail(&to, body, true);
    }

    #[allow(dead_code)] // wired up with the message menu next
    pub fn delete_msg(&mut self, key: &str, mid: &str) {
        let own = self.me.msgs.get(key).and_then(|v| v.iter().find(|m| m.mid == mid)).map(|m| m.uid == self.me.uid).unwrap_or(false);
        if !own {
            return;
        }
        let mut body = json!({"t": "op", "ev": {"type": "del", "mid": mid}});
        if key.starts_with("g:") {
            body["k"] = Self::key_part(key);
        }
        let to = self.dm_targets(key);
        self.mail(&to, body, false);
        if let Some(v) = self.me.msgs.get_mut(key) {
            v.retain(|m| m.mid != mid);
        }
        self.save();
    }

    fn add_msg(&mut self, key: &str, m: Msg) {
        let v = self.me.msgs.entry(key.to_string()).or_default();
        if !m.mid.is_empty() && v.iter().any(|x| x.mid == m.mid && x.uid == m.uid) {
            return;
        }
        let ts = m.ts;
        // Keep them in time order, the way the web app does when offline mail arrives late.
        let at = v.iter().rposition(|x| x.ts <= ts).map(|i| i + 1).unwrap_or(0);
        v.insert(at, m);
        if v.len() > DM_HISTORY {
            let extra = v.len() - DM_HISTORY;
            v.drain(..extra);
        }
        if let Some(d) = self.me.dms.get_mut(key) {
            d.last = d.last.max(ts);
            d.closed = false;
        }
        self.save();
    }

    fn sys_line(&mut self, key: &str, text: String, ts: i64) {
        self.add_msg(key, Msg { mid: rid(6), text, ts, sys: true, ..Default::default() });
    }

    /* ---------- what arrives from the mailbox ---------- */

    /// Handles one message from the Inbox WebSocket. Returns the DM keys that got new messages.
    pub fn on_text(&mut self, raw: &str) -> Vec<String> {
        let Ok(m) = serde_json::from_str::<Value>(raw) else { return vec![] };
        match m["type"].as_str() {
            Some("presence") => {
                self.on_presence(&m);
                vec![]
            }
            Some("mail") => {
                let mut ack = Vec::new();
                let mut touched = Vec::new();
                for it in m["items"].as_array().cloned().unwrap_or_default() {
                    if let Some(id) = it["id"].as_i64() {
                        ack.push(id);
                        if !self.seen_mail.insert(id) {
                            continue;
                        }
                    }
                    let from = it["from"].as_str().unwrap_or_default().to_string();
                    let b = &it["body"];
                    if !is_uid(&from) || !b.is_object() || self.me.friends.get(&from).map(|f| f.state) == Some(FriendState::Blocked) {
                        continue;
                    }
                    let ts = it["ts"].as_i64().unwrap_or_else(now_ms);
                    if let Some(k) = self.handle_mail(&from, b, ts) {
                        touched.push(k);
                    }
                }
                if !ack.is_empty() {
                    self.outbox.push(json!({"type": "ack", "ids": ack}).to_string());
                }
                self.save();
                touched
            }
            _ => vec![],
        }
    }

    fn on_presence(&mut self, m: &Value) {
        let list = m["list"].as_object().cloned().unwrap_or_default();
        if m["all"].as_bool() == Some(true) {
            self.presence.retain(|u, _| list.contains_key(u));
        }
        for (u, p) in list {
            if !is_uid(&u) {
                continue;
            }
            let p: Option<Presence> = if p.is_object() { serde_json::from_value(p).ok() } else { None };
            if let (Some(p), Some(f)) = (&p, self.me.friends.get_mut(&u)) {
                let nm: String = p.name.chars().take(32).collect();
                if !nm.is_empty() && f.name != nm {
                    f.name = nm;
                }
            }
            self.presence.insert(u, p);
        }
    }

    fn mail_key(from: &str, b: &Value) -> String {
        match b["k"].as_str() {
            Some(k) if is_gid(k) => format!("g:{k}"),
            _ => format!("u:{from}"),
        }
    }

    fn handle_mail(&mut self, from: &str, b: &Value, ts: i64) -> Option<String> {
        let nm = one_line(b.get("n"), 32);
        match b["t"].as_str()? {
            "freq" => {
                let state = self.me.friends.get(from).map(|f| f.state);
                if matches!(state, Some(FriendState::Friend) | Some(FriendState::Out)) {
                    if state == Some(FriendState::Out) {
                        self.notices.push(Notice::Info(format!("You and {} are now friends!", if nm.is_empty() { self.name_of(from) } else { nm.clone() })));
                    }
                    let name = if nm.is_empty() { self.name_of(from) } else { nm };
                    self.me.friends.insert(from.into(), Friend { name, state: FriendState::Friend, since: now_ms() });
                    let me = self.me.name.clone();
                    self.mail(&[from.into()], json!({"t": "facc", "n": me}), false);
                    self.friends_changed();
                } else if state != Some(FriendState::In) {
                    let name = if nm.is_empty() { "Someone".to_string() } else { nm };
                    self.notices.push(Notice::Info(format!("{name} sent you a friend request.")));
                    self.me.friends.insert(from.into(), Friend { name, state: FriendState::In, since: ts });
                }
                None
            }
            "facc" => {
                let f = self.me.friends.get(from)?;
                if f.state == FriendState::Blocked {
                    return None;
                }
                let name = if nm.is_empty() { f.name.clone() } else { nm };
                if f.state == FriendState::Out {
                    self.notices.push(Notice::Info(format!("{name} accepted your friend request!")));
                }
                self.me.friends.insert(from.into(), Friend { name, state: FriendState::Friend, since: now_ms() });
                self.friends_changed();
                None
            }
            "fdel" => {
                if self.me.friends.get(from).map(|f| f.state) != Some(FriendState::Blocked) && self.me.friends.remove(from).is_some() {
                    self.presence.remove(from);
                    self.friends_changed();
                }
                None
            }
            "dm" => self.mail_dm(from, b, nm, ts),
            "op" => self.mail_op(from, b),
            "typing" => {
                let key = Self::mail_key(from, b);
                let d = self.me.dms.get(&key)?;
                if d.group && !d.members.contains_key(from) {
                    return None;
                }
                let name = if nm.is_empty() { self.name_of(from) } else { nm };
                self.typing.insert(from.into(), (key, name, now_ms() + 6000));
                None
            }
            "gmeta" => self.mail_group(from, b),
            "ring" => {
                let name = if nm.is_empty() { self.name_of(from) } else { nm };
                self.notices.push(Notice::Info(format!("{name} is calling you. Calls aren't in the desktop app yet, so answer in the browser.")));
                None
            }
            _ => None, // files and calls come later
        }
    }

    fn clean_group(g: &Value) -> Option<(String, BTreeMap<String, String>, String)> {
        let mut members = BTreeMap::new();
        for (u, n) in g["members"].as_object()?.iter().take(GROUP_MAX) {
            if is_uid(u) {
                let n = one_line(Some(n), 32);
                members.insert(u.clone(), if n.is_empty() { "Someone".into() } else { n });
            }
        }
        if members.is_empty() {
            return None;
        }
        let owner = g["owner"].as_str().filter(|o| members.contains_key(*o)).map(String::from).unwrap_or_else(|| members.keys().next().unwrap().clone());
        Some((one_line(g.get("name"), 100), members, owner))
    }

    fn group_from_mail(&mut self, key: &str, b: &Value, from: &str) -> bool {
        let Some((name, members, owner)) = Self::clean_group(&b["g"]) else { return false };
        if !members.contains_key(&self.me.uid) || !members.contains_key(from) {
            return false;
        }
        self.me.dms.insert(key.into(), Dm { key: key.into(), group: true, cid: key[2..].into(), name, members, owner, last: now_ms(), ..Default::default() });
        true
    }

    fn mail_dm(&mut self, from: &str, b: &Value, nm: String, ts: i64) -> Option<String> {
        let key = Self::mail_key(from, b);
        let m = &b["msg"];
        let mid = m["mid"].as_str().unwrap_or_default().to_string();
        if !m.is_object() || !is_mid(&mid) {
            return None;
        }
        if key.starts_with("g:") {
            if !self.me.dms.contains_key(&key) {
                if !self.group_from_mail(&key, b, from) {
                    return None;
                }
                let who = if nm.is_empty() { "Someone".to_string() } else { nm.clone() };
                self.sys_line(&key, format!("{who} added you to the group."), ts - 1);
            }
            let d = self.me.dms.get(&key)?;
            if !d.members.contains_key(from) || d.left {
                return None;
            }
        } else {
            let cid = b["cid"].as_str().filter(|c| is_gid(c)).map(String::from);
            let d = self.me.dms.entry(key.clone()).or_insert_with(|| Dm {
                key: key.clone(),
                members: BTreeMap::from([(from.to_string(), if nm.is_empty() { "Someone".into() } else { nm.clone() })]),
                cid: cid.clone().unwrap_or_else(|| rid(12)),
                ..Default::default()
            });
            // Both started a DM at once: settle on one call room, the same way the web app does.
            if let Some(c) = cid {
                if c < d.cid {
                    d.cid = c;
                }
            }
        }
        if !nm.is_empty() {
            if let Some(d) = self.me.dms.get_mut(&key) {
                d.members.insert(from.into(), nm.clone());
            }
        }
        let mut text: String = m["text"].as_str().unwrap_or_default().chars().take(2000).collect();
        let file = m["file"]["name"].as_str().map(|n| n.chars().take(120).collect::<String>());
        if text.is_empty() && file.is_none() {
            text = String::new();
        }
        let msg = Msg {
            mid,
            uid: from.into(),
            name: if nm.is_empty() { self.name_of(from) } else { nm },
            text,
            ts: m["ts"].as_i64().unwrap_or(ts).min(now_ms()),
            file,
            ..Default::default()
        };
        self.typing.remove(from);
        self.add_msg(&key, msg);
        Some(key)
    }

    fn mail_op(&mut self, from: &str, b: &Value) -> Option<String> {
        let key = Self::mail_key(from, b);
        let d = self.me.dms.get(&key)?;
        if d.group && !d.members.contains_key(from) {
            return None;
        }
        let e = &b["ev"];
        let mid = e["mid"].as_str()?;
        let v = self.me.msgs.get_mut(&key)?;
        let i = v.iter().position(|m| m.mid == mid)?;
        if v[i].uid != from {
            return None; // only your own messages can be edited or deleted
        }
        match e["type"].as_str()? {
            "edit" => {
                v[i].text = e["text"].as_str().unwrap_or_default().chars().take(2000).collect();
                v[i].edited = Some(now_ms());
            }
            "del" => {
                v.remove(i);
            }
            _ => return None,
        }
        Some(key)
    }

    fn mail_group(&mut self, from: &str, b: &Value) -> Option<String> {
        let key = Self::mail_key(from, b);
        if !key.starts_with("g:") {
            return None;
        }
        if !self.me.dms.contains_key(&key) {
            if !self.group_from_mail(&key, b, from) {
                return None;
            }
            let who = self.name_of(from);
            self.sys_line(&key, format!("{who} added you to the group."), now_ms());
            return Some(key);
        }
        let (name, members, owner) = Self::clean_group(&b["g"])?;
        let d = self.me.dms.get(&key)?.clone();
        if !d.members.contains_key(from) || d.left {
            return None;
        }
        let who = d.members.get(from).cloned().unwrap_or_else(|| "Someone".into());
        let added: Vec<String> = members.keys().filter(|u| !d.members.contains_key(*u)).cloned().collect();
        let gone: Vec<String> = d.members.keys().filter(|u| !members.contains_key(*u)).cloned().collect();
        let leaving = b["why"].as_str() == Some("leave");
        // Only the owner removes people; leaving means only you go.
        let bad = if leaving { gone.len() != 1 || gone[0] != from || !added.is_empty() } else { !gone.is_empty() && from != d.owner };
        if bad {
            return None;
        }
        let now = now_ms();
        if name != d.name {
            let line = if name.is_empty() { format!("{who} removed the group name.") } else { format!("{who} changed the group name to {name}.") };
            self.sys_line(&key, line, now);
        }
        for u in &added {
            self.sys_line(&key, format!("{who} added {} to the group.", members[u]), now);
        }
        for u in &gone {
            let line = if u == from { format!("{who} left the group.") } else { format!("{who} removed {} from the group.", d.members[u]) };
            self.sys_line(&key, line, now);
        }
        let me = self.me.uid.clone();
        let removed = !members.contains_key(&me);
        if let Some(d) = self.me.dms.get_mut(&key) {
            d.name = name;
            d.members = members;
            d.owner = owner;
            if removed {
                d.left = true;
            }
        }
        if removed {
            self.sys_line(&key, "You were removed from this group.".into(), now);
        }
        Some(key)
    }

    /// Who is typing in this DM right now.
    pub fn typing_in(&self, key: &str) -> Vec<String> {
        let now = now_ms();
        self.typing.values().filter(|(k, _, until)| k == key && *until > now).map(|(_, n, _)| n.clone()).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store() -> Store {
        let me = Saved { uid: "aaaaaaaaaaaaaaaa".into(), key: "k".repeat(24), name: "Me".into(), ..Default::default() };
        Store::from_saved(me, std::env::temp_dir().join(format!("lc-test-{}.json", rid(4))))
    }
    fn mail(from: &str, id: i64, body: Value) -> String {
        json!({"type": "mail", "items": [{"id": id, "from": from, "ts": now_ms(), "body": body}]}).to_string()
    }
    const BOB: &str = "bbbbbbbbbbbbbbbb";

    #[test]
    fn friend_request_then_dm() {
        let mut s = store();
        s.on_text(&mail(BOB, 1, json!({"t": "freq", "n": "Bob"})));
        assert_eq!(s.me.friends[BOB].state, FriendState::In);
        assert!(s.outbox.iter().any(|o| o.contains("\"ack\"")));
        s.accept_friend(BOB);
        assert_eq!(s.me.friends[BOB].state, FriendState::Friend);
        assert!(s.outbox.iter().any(|o| o.contains("facc")));
        let k = s.on_text(&mail(BOB, 2, json!({"t": "dm", "n": "Bob", "cid": "c".repeat(24), "msg": {"mid": "0123456789ab", "text": "hi", "ts": now_ms()}})));
        assert_eq!(k, vec![format!("u:{BOB}")]);
        assert_eq!(s.me.msgs[&k[0]][0].text, "hi");
        // The same mail again (another tab reconnecting) is ignored.
        s.on_text(&mail(BOB, 2, json!({"t": "dm", "n": "Bob", "msg": {"mid": "0123456789ab", "text": "hi", "ts": now_ms()}})));
        assert_eq!(s.me.msgs[&k[0]].len(), 1);
        // Bob deletes it.
        s.on_text(&mail(BOB, 3, json!({"t": "op", "ev": {"type": "del", "mid": "0123456789ab"}})));
        assert!(s.me.msgs[&k[0]].is_empty());
    }

    #[test]
    fn sending_matches_web_format() {
        let mut s = store();
        s.me.friends.insert(BOB.into(), Friend { name: "Bob".into(), state: FriendState::Friend, since: 0 });
        let key = s.dm_for(BOB);
        s.outbox.clear();
        s.send_text(&key, "  hello  ");
        let v: Value = serde_json::from_str(&s.outbox[0]).unwrap();
        assert_eq!(v["type"], "send");
        assert_eq!(v["to"][0], BOB);
        assert_eq!(v["body"]["t"], "dm");
        assert_eq!(v["body"]["msg"]["text"], "hello");
        assert_eq!(v["body"]["cid"].as_str().unwrap().len(), 24);
        assert_eq!(v["body"]["msg"]["mid"].as_str().unwrap().len(), 12);
    }

    #[test]
    fn link_code_from_web() {
        // Made the way the web app's "Copy Link Code" button makes it: base64 of the UTF-8 JSON.
        let dir = std::env::temp_dir().join(format!("lc-link-{}", rid(4)));
        std::env::set_var("LITECORD_DATA", &dir);
        let json = json!({"v": 1, "uid": "cccccccccccccccc", "key": "x".repeat(32), "name": "ゆき",
            "friends": {BOB: {"name": "Bob", "state": "friend", "since": 1}},
            "dms": {format!("u:{BOB}"): {"key": format!("u:{BOB}"), "group": false, "members": {BOB: "Bob"}, "cid": "d".repeat(24), "last": 5}}});
        let code = format!("lc1:{}", base64::engine::general_purpose::STANDARD.encode(json.to_string()));
        let s = Store::link(&code, "fallback").unwrap();
        assert_eq!(s.me.name, "ゆき");
        assert_eq!(s.me.uid, "cccccccccccccccc");
        assert_eq!(s.friend_uids(), vec![BOB.to_string()]);
        assert_eq!(s.dm_title(&format!("u:{BOB}")), "Bob");
        assert!(Store::link("hello", "x").is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn group_invite() {
        let mut s = store();
        let g = json!({"name": "", "members": {BOB: "Bob", "aaaaaaaaaaaaaaaa": "Me"}, "owner": BOB});
        let gid = "0123456789abcdef01234567";
        s.on_text(&mail(BOB, 1, json!({"t": "gmeta", "n": "Bob", "k": gid, "g": g, "why": "create"})));
        let key = format!("g:{gid}");
        assert!(s.me.dms[&key].group);
        assert_eq!(s.dm_title(&key), "Bob");
    }
}

//! The window: title bar, server rail, sidebar (Friends + Direct Messages + your user bar) and the
//! main area (the Friends page or a DM). Settings live in settings.rs, the title bar in titlebar.rs.
//! Every size goes through u() so the UI scale setting grows or shrinks all of it, and every color
//! comes from the current theme (prefs.rs).

use std::rc::Rc;
use std::time::Duration;

use chrono::{Local, TimeZone};
use gpui_kit::component::button::*;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::message_scroller::{MessageScroller, MessageScrollerState};
use gpui_kit::component::theme::{Theme, ThemeMode};
use gpui_kit::component::*;
use gpui_kit::*;
use gpui_kit::{prelude::FluentBuilder as _, white};

use crate::net::{self, NetEvent, NetHandle};
use crate::prefs::*;
use crate::settings::Page;
use crate::store::{now_ms, FriendState, Msg, Notice, Store};

pub const SERVER: &str = "https://litecord.litecord.workers.dev";

actions!(litecord, [ZoomIn, ZoomOut, ZoomReset, ToggleSettings, CloseOverlay]);

pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new("ctrl-=", ZoomIn, None),
        KeyBinding::new("ctrl-+", ZoomIn, None),
        KeyBinding::new("ctrl-shift-=", ZoomIn, None),
        KeyBinding::new("ctrl--", ZoomOut, None),
        KeyBinding::new("ctrl-0", ZoomReset, None),
        KeyBinding::new("ctrl-,", ToggleSettings, None),
        KeyBinding::new("escape", CloseOverlay, None),
    ]);
}

/// The web app's hue(): the same person gets the same avatar color everywhere.
pub fn hue(s: &str) -> f32 {
    let mut n: u32 = 0;
    for c in s.encode_utf16() {
        n = (n * 31 + c as u32) % 360;
    }
    n as f32
}

pub fn status_color(s: &str) -> Hsla {
    match s {
        "online" => c_green(),
        "idle" => c_yellow(),
        "dnd" => c_red(),
        _ => rgb(0x80848e).into(),
    }
}

pub fn status_label(s: &str) -> &'static str {
    match s {
        "online" => "Online",
        "idle" => "Idle",
        "dnd" => "Do Not Disturb",
        "invisible" => "Invisible",
        _ => "Offline",
    }
}

pub fn avatar(uid: &str, name: &str, size: f32) -> Div {
    let letter: String = name.chars().next().unwrap_or('?').to_uppercase().collect();
    div()
        .flex_none()
        .size(u(size))
        .rounded_full()
        .bg(hsla(hue(uid) / 360., 0.45, 0.40, 1.))
        .flex()
        .items_center()
        .justify_center()
        .text_color(white())
        .text_size(u(size * 0.42))
        .font_weight(FontWeight::SEMIBOLD)
        .child(letter)
}

/// An avatar with the little status dot in the corner, cut out of the background like Discord's.
pub fn avatar_dot(uid: &str, name: &str, size: f32, status: &str, ring: Hsla) -> Div {
    let dot = size * 0.32;
    div().relative().flex_none().size(u(size)).child(avatar(uid, name, size)).child(
        div()
            .absolute()
            .right(u(-2.))
            .bottom(u(-2.))
            .size(u(dot + 5.))
            .rounded_full()
            .bg(ring)
            .flex()
            .items_center()
            .justify_center()
            .child(div().size(u(dot)).rounded_full().bg(status_color(status))),
    )
}

fn when(ts: i64, h24: bool) -> String {
    let Some(t) = Local.timestamp_millis_opt(ts).single() else { return String::new() };
    let today = Local::now().date_naive();
    let day = t.date_naive();
    let clock = clock(ts, h24);
    if day == today {
        format!("Today at {clock}")
    } else if Some(day) == today.pred_opt() {
        format!("Yesterday at {clock}")
    } else {
        format!("{} {clock}", t.format("%m/%d/%Y"))
    }
}

fn clock(ts: i64, h24: bool) -> String {
    let Some(t) = Local.timestamp_millis_opt(ts).single() else { return String::new() };
    if h24 { t.format("%H:%M").to_string() } else { t.format("%-I:%M %p").to_string() }
}

#[derive(Clone, PartialEq)]
pub enum View {
    Friends(Tab),
    Dm(String),
    /// A server (relay room) by name.
    Server(String),
}

#[derive(Clone, Copy, PartialEq)]
pub enum Tab {
    Online,
    All,
    Pending,
    Blocked,
    Add,
}

/// One line in the chat, worked out once per change so the list can draw rows quickly.
pub struct Row {
    msg: Msg,
    head: bool,          // first of a group: show avatar and name
    day: Option<String>, // a date divider above this row
}

pub struct LiteCord {
    pub(crate) store: Option<Store>,
    pub(crate) net: Option<NetHandle>,
    pub(crate) view: View,
    pub(crate) prefs: Prefs,
    prefs_dirty: bool,
    /// The buttons this desktop shows on title bars (left side, right side).
    pub(crate) detected: (Vec<Ctl>, Vec<Ctl>),
    /// Windows can't switch frames on the fly, so remember what the window was opened with.
    pub(crate) native_at_start: bool,
    pub(crate) settings: Option<Page>,
    pub(crate) status_menu: bool,
    pub(crate) focus: FocusHandle,
    // setup screen
    name_input: Entity<InputState>,
    link_input: Entity<InputState>,
    setup_error: Option<String>,
    // main window
    composer: Entity<InputState>,
    add_input: Entity<InputState>,
    pub(crate) custom_input: Entity<InputState>,
    pub(crate) rename_input: Entity<InputState>,
    pub(crate) username_input: Entity<InputState>,
    pub(crate) settings_search: Entity<InputState>,
    pub(crate) account_msg: Option<(bool, String)>,
    pub(crate) logout_armed: bool,
    add_result: Option<(bool, String)>,
    scroller: Entity<MessageScrollerState>,
    rows: Rc<Vec<Row>>,
    toast: Option<(String, i64)>,
    pub(crate) last_typing: i64,
    active: bool,
    pub(crate) drag_armed: bool,
    window_title: String,
    pub(crate) call: crate::callui::CallUi,
    /// Video pictures that were replaced, to free on the GPU at the next frame.
    pub(crate) old_images: Vec<std::sync::Arc<RenderImage>>,
    pub(crate) server: crate::servers::ServerUi,
    pub(crate) server_name_input: Entity<InputState>,
    pub(crate) server_composer: Entity<InputState>,
    pub(crate) server_scroller: Entity<MessageScrollerState>,
    pub(crate) server_rows: Rc<Vec<(crate::call::ChatView, bool)>>,
    pub(crate) server_rows_for: Option<String>,
    pub(crate) server_rows_len: usize,
    /// Microphones and speakers found (for Settings › Voice & Video).
    pub(crate) audio_devices: Option<(Vec<String>, Vec<String>)>,
    pub(crate) voice: crate::voiceui::VoiceUi,
    pub(crate) update: crate::update::UpdateUi,
    _subs: Vec<Subscription>,
}

impl LiteCord {
    pub fn new(prefs: Prefs, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let name_input = cx.new(|cx| InputState::new(window, cx).placeholder("What should everyone call you?"));
        let link_input = cx.new(|cx| InputState::new(window, cx).placeholder("lc1:…"));
        let composer = cx.new(|cx| InputState::new(window, cx).placeholder("Message"));
        let add_input = cx.new(|cx| InputState::new(window, cx).placeholder("You can add friends with their LiteCord username or friend code."));
        let custom_input = cx.new(|cx| InputState::new(window, cx).placeholder("Set a custom status").default_value(prefs.custom.clone()));
        let rename_input = cx.new(|cx| InputState::new(window, cx).placeholder("Display name"));
        let username_input = cx.new(|cx| InputState::new(window, cx).placeholder("username"));
        let settings_search = cx.new(|cx| InputState::new(window, cx).placeholder("Search"));
        let scroller = cx.new(|cx| MessageScrollerState::new(0, cx));
        let server_name_input = cx.new(|cx| InputState::new(window, cx).placeholder("lobby"));
        let server_composer = cx.new(|cx| InputState::new(window, cx).placeholder("Message"));
        let server_scroller = cx.new(|cx| MessageScrollerState::new(0, cx));

        let mut subs = Vec::new();
        subs.push(cx.subscribe_in(&composer, window, |this, input, ev: &InputEvent, window, cx| match ev {
            InputEvent::PressEnter { shift: false, .. } => {
                let text = input.read(cx).value().to_string();
                if let (View::Dm(key), Some(store)) = (this.view.clone(), this.store.as_mut()) {
                    if !text.trim().is_empty() {
                        store.send_text(&key, &text);
                        input.update(cx, |s, cx| s.set_value("", window, cx));
                        this.after_change(cx);
                    }
                }
            }
            InputEvent::Change => {
                // Tell them we're typing, at most every 4 seconds (like the web app).
                let now = now_ms();
                if now - this.last_typing > 4000 && !input.read(cx).value().is_empty() {
                    if let (View::Dm(key), Some(store)) = (this.view.clone(), this.store.as_mut()) {
                        this.last_typing = now;
                        store.send_typing(&key);
                        this.flush();
                    }
                }
            }
            _ => {}
        }));
        subs.push(cx.subscribe_in(&server_name_input, window, |this, _, ev: &InputEvent, window, cx| {
            if let InputEvent::PressEnter { .. } = ev {
                this.submit_add_server(window, cx);
            }
        }));
        subs.push(cx.subscribe_in(&server_composer, window, |this, input, ev: &InputEvent, window, cx| match ev {
            InputEvent::PressEnter { shift: false, .. } => this.send_server_message(input, window, cx),
            InputEvent::Change if !input.read(cx).value().is_empty() => this.server_typing(cx),
            _ => {}
        }));
        subs.push(cx.subscribe_in(&add_input, window, |this, _, ev: &InputEvent, window, cx| {
            if let InputEvent::PressEnter { .. } = ev {
                this.send_friend_request(window, cx);
            }
        }));
        subs.push(cx.subscribe_in(&name_input, window, |this, _, ev: &InputEvent, _, cx| {
            if let InputEvent::PressEnter { .. } = ev {
                this.start_fresh(cx);
            }
        }));
        subs.push(cx.subscribe_in(&custom_input, window, |this, input, ev: &InputEvent, _, cx| {
            if let InputEvent::PressEnter { .. } = ev {
                let text = input.read(cx).value().to_string();
                let status = this.prefs.status.clone();
                this.set_status(&status, &text, cx);
                this.status_menu = false;
                cx.notify();
            }
        }));
        subs.push(cx.subscribe_in(&settings_search, window, |_, _, ev: &InputEvent, _, cx| {
            if let InputEvent::Change = ev {
                cx.notify();
            }
        }));
        subs.push(cx.observe_window_activation(window, |this, window, cx| {
            this.active = window.is_window_active();
            if this.active {
                if let (View::Dm(k), Some(s)) = (this.view.clone(), this.store.as_mut()) {
                    s.unread.remove(&k);
                }
                cx.notify();
            }
        }));
        // Remember the window size for next time (saved by the slow tick, not on every pixel).
        subs.push(cx.observe_window_bounds(window, |this, window, _| {
            match window.window_bounds() {
                WindowBounds::Windowed(b) => {
                    this.prefs.win_w = f32::from(b.size.width);
                    this.prefs.win_h = f32::from(b.size.height);
                    this.prefs.maximized = false;
                }
                WindowBounds::Maximized(_) => this.prefs.maximized = true,
                _ => {}
            }
            this.prefs_dirty = true;
        }));

        let focus = cx.focus_handle();
        window.focus(&focus, cx);
        let native_at_start = prefs.native_frame;
        let mut app = LiteCord {
            store: Store::load(),
            net: None,
            view: View::Friends(Tab::Online),
            prefs,
            prefs_dirty: false,
            detected: detect_buttons(),
            native_at_start,
            settings: None,
            status_menu: false,
            focus,
            name_input,
            link_input,
            setup_error: None,
            composer,
            add_input,
            custom_input,
            rename_input,
            username_input,
            settings_search,
            account_msg: None,
            logout_armed: false,
            add_result: None,
            scroller,
            rows: Rc::new(Vec::new()),
            toast: None,
            last_typing: 0,
            active: true,
            drag_armed: false,
            window_title: String::new(),
            call: Default::default(),
            old_images: Vec::new(),
            server: Default::default(),
            server_name_input,
            server_composer,
            server_scroller,
            server_rows: Rc::new(Vec::new()),
            server_rows_for: None,
            server_rows_len: 0,
            audio_devices: None,
            voice: Default::default(),
            update: Default::default(),
            _subs: subs,
        };
        app.apply_look(cx);
        if app.store.is_some() {
            app.started(cx);
            // For screenshots and testing: LITECORD_SETTINGS=appearance opens that settings page.
            if let Ok(p) = std::env::var("LITECORD_SETTINGS") {
                if let Some(page) = Page::from_name(&p) {
                    app.open_settings(page, window, cx);
                }
            }
        }
        // A slow tick: clears "is typing…" and old toasts, and saves the window size.
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_millis(1000)).await;
            if this.update(cx, |this, cx| this.tick(cx)).is_err() {
                break;
            }
        })
        .detach();
        app
    }

    /// Called once there's an account: connect, and reopen the last conversation.
    fn started(&mut self, cx: &mut Context<Self>) {
        let (status, custom) = (self.prefs.status.clone(), self.prefs.custom.clone());
        let Some(store) = self.store.as_mut() else { return };
        store.status = status;
        store.custom = custom;
        let last = store.me.last_view.clone();
        if store.me.dms.contains_key(&last) {
            store.unread.remove(&last);
            self.view = View::Dm(last);
        }
        self.connect(cx);
        self.rebuild_rows(cx);
    }

    fn connect(&mut self, cx: &mut Context<Self>) {
        let Some(store) = &self.store else { return };
        let (tx, rx) = async_channel::unbounded();
        self.net = Some(net::start(store.inbox_url(SERVER), tx));
        cx.spawn(async move |this, cx| {
            while let Ok(ev) = rx.recv().await {
                if this.update(cx, |this, cx| this.on_net(ev, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    /// Applies the theme and scale to everything, including the input boxes and buttons.
    pub(crate) fn apply_look(&mut self, cx: &mut Context<Self>) {
        set_theme(&self.prefs.theme);
        set_scale(self.prefs.zoom);
        Theme::change(if pal().light { ThemeMode::Light } else { ThemeMode::Dark }, None, cx);
        // Blurple main buttons, like the web app (GPUI Kit's dark theme makes them white).
        Theme::update(cx, |t| {
            t.primary = c_accent();
            t.primary_hover = rgb(0x4752c4).into();
            t.primary_active = rgb(0x3c45a5).into();
            t.primary_foreground = white();
        });
        self.scroller.update(cx, |s, cx| s.remeasure(cx));
        cx.notify();
    }

    pub(crate) fn save_prefs(&mut self) {
        self.prefs.save();
        self.prefs_dirty = false;
    }

    pub(crate) fn set_zoom(&mut self, zoom: u32, cx: &mut Context<Self>) {
        self.prefs.zoom = zoom.clamp(50, 200);
        self.save_prefs();
        self.apply_look(cx);
    }

    pub(crate) fn set_status(&mut self, status: &str, custom: &str, cx: &mut Context<Self>) {
        self.prefs.status = status.to_string();
        self.prefs.custom = custom.trim().chars().take(128).collect();
        self.save_prefs();
        if let Some(s) = self.store.as_mut() {
            s.set_status(status, &self.prefs.custom.clone());
        }
        self.after_change(cx);
    }

    fn on_net(&mut self, ev: NetEvent, cx: &mut Context<Self>) {
        let Some(store) = self.store.as_mut() else { return };
        match ev {
            NetEvent::Connected => {
                store.online = true;
                let hello = store.hello();
                if let Some(n) = &self.net {
                    n.send(hello);
                }
            }
            NetEvent::Disconnected { replaced } => {
                store.online = false;
                if replaced {
                    self.toast("LiteCord is open in too many places, so this window disconnected.", 8000);
                }
            }
            NetEvent::Text(t) => {
                let was_ringing = store.ringing.as_ref().map(|r| r.from.clone());
                // Friend requests in, and ours that were accepted, play the friend sound.
                let friendish = |st: &Store| st.me.friends.values().filter(|f| matches!(f.state, FriendState::In | FriendState::Friend)).count();
                let friends_before = friendish(store);
                let touched = store.on_text(&t);
                let friend_sound = friendish(store) > friends_before;
                if let Some(r) = store.ringing.as_ref().filter(|r| Some(&r.from) != was_ringing.as_ref()) {
                    if self.prefs.notify && self.prefs.status != "dnd" {
                        let body = if r.video { format!("{} is video calling you", r.name) } else { format!("{} is calling you", r.name) };
                        std::thread::spawn(move || {
                            let _ = notify_rust::Notification::new().appname("LiteCord").summary("Incoming call").body(&body).show();
                        });
                    }
                }
                let mut alert = Vec::new();
                for k in touched {
                    let looking = self.active && self.view == View::Dm(k.clone()) && self.settings.is_none();
                    if !looking {
                        *store.unread.entry(k.clone()).or_default() += 1;
                        alert.push(k);
                    }
                }
                let message_sound = alert.iter().any(|k| self.from_someone_else(k));
                for k in alert {
                    self.notify_message(&k);
                }
                if friend_sound {
                    self.sfx("friend");
                }
                if message_sound && self.prefs.status != "dnd" {
                    self.sfx("message");
                }
            }
        }
        self.after_change(cx);
    }

    /// Whether a DM's newest message is a real one from someone else.
    fn from_someone_else(&self, key: &str) -> bool {
        let Some(store) = &self.store else { return false };
        store.me.msgs.get(key).and_then(|v| v.last()).is_some_and(|m| !m.sys && m.uid != store.me.uid)
    }

    /// Plays a sound effect, unless it's turned off in Settings.
    pub(crate) fn sfx(&self, name: &str) {
        if self.prefs.sounds_off.iter().any(|s| s == crate::sfx::setting_of(name)) {
            return;
        }
        crate::sfx::play(name, &self.prefs.speaker_device, self.prefs.sfx_vol);
    }

    /// A desktop notification for a new DM, unless you're looking at it or on Do Not Disturb.
    fn notify_message(&self, key: &str) {
        if !self.prefs.notify || self.prefs.status == "dnd" {
            return;
        }
        let Some(store) = &self.store else { return };
        let Some(m) = store.me.msgs.get(key).and_then(|v| v.last()) else { return };
        if m.sys || m.uid == store.me.uid {
            return;
        }
        let title = if key.starts_with("g:") { format!("{} ({})", m.name, store.dm_title(key)) } else { m.name.clone() };
        let body = if !self.prefs.notify_preview {
            "Sent you a message".to_string()
        } else if m.text.is_empty() {
            m.file.as_ref().map(|f| format!("📎 {f}")).unwrap_or_default()
        } else {
            m.text.chars().take(200).collect()
        };
        std::thread::spawn(move || {
            let _ = notify_rust::Notification::new().appname("LiteCord").summary(&title).body(&body).show();
        });
    }

    /// Sends what the store queued, shows its notices, and redraws.
    pub(crate) fn after_change(&mut self, cx: &mut Context<Self>) {
        self.flush();
        let notices: Vec<Notice> = self.store.as_mut().map(|s| std::mem::take(&mut s.notices)).unwrap_or_default();
        for Notice::Info(t) in notices {
            self.toast(&t, 5000);
        }
        self.rebuild_rows(cx);
        cx.notify();
    }

    fn flush(&mut self) {
        let (Some(store), Some(net)) = (self.store.as_mut(), &self.net) else { return };
        for s in store.outbox.drain(..) {
            net.send(s);
        }
    }

    pub(crate) fn toast(&mut self, text: &str, ms: i64) {
        self.toast = Some((text.to_string(), now_ms() + ms));
    }

    fn tick(&mut self, cx: &mut Context<Self>) {
        let now = now_ms();
        let mut dirty = false;
        if self.toast.as_ref().is_some_and(|(_, until)| *until < now) {
            self.toast = None;
            dirty = true;
        }
        if let Some(s) = self.store.as_mut() {
            let before = s.typing.len();
            s.typing.retain(|_, (_, _, until)| *until > now);
            dirty |= before != s.typing.len();
        }
        if self.prefs_dirty {
            self.save_prefs();
        }
        dirty |= self.call_tick();
        if dirty {
            cx.notify();
        }
    }

    fn rebuild_rows(&mut self, cx: &mut Context<Self>) {
        let View::Dm(key) = &self.view else { return };
        let Some(store) = &self.store else { return };
        let msgs = store.me.msgs.get(key).cloned().unwrap_or_default();
        let mut rows = Vec::with_capacity(msgs.len());
        let mut prev: Option<&Msg> = None;
        for m in &msgs {
            let day = Local.timestamp_millis_opt(m.ts).single().map(|t| t.date_naive());
            let prev_day = prev.and_then(|p| Local.timestamp_millis_opt(p.ts).single()).map(|t| t.date_naive());
            let new_day = day != prev_day;
            // Like Discord: the same person within 7 minutes joins the group above.
            let head = new_day || m.sys || prev.map_or(true, |p| p.sys || p.uid != m.uid || m.ts - p.ts > 7 * 60_000);
            rows.push(Row { msg: m.clone(), head, day: if new_day { day.map(|d| d.format("%B %-d, %Y").to_string()) } else { None } });
            prev = Some(m);
        }
        let old = self.rows.len();
        let new = rows.len();
        self.rows = Rc::new(rows);
        self.scroller.update(cx, |s, cx| {
            if s.item_count() == old && new > old {
                s.append(new - old, cx);
            } else {
                s.reset(new, cx);
            }
        });
    }

    pub(crate) fn open_view(&mut self, v: View, window: &mut Window, cx: &mut Context<Self>) {
        if let (View::Dm(k), Some(s)) = (&v, self.store.as_mut()) {
            s.unread.remove(k);
            if let Some(d) = s.me.dms.get_mut(k) {
                d.closed = false;
            }
        }
        let switching = self.view != v;
        self.view = v;
        self.status_menu = false;
        if let Some(s) = self.store.as_mut() {
            s.me.last_view = match &self.view {
                View::Dm(k) => k.clone(),
                _ => String::new(),
            };
            s.save();
        }
        if switching {
            self.rows = Rc::new(Vec::new());
            self.scroller.update(cx, |s, cx| s.reset(0, cx));
            self.add_result = None;
        }
        self.rebuild_rows(cx);
        if let View::Dm(k) = &self.view {
            let title = self.store.as_ref().map(|s| s.dm_title(k)).unwrap_or_default();
            let ph = if k.starts_with("g:") { format!("Message {title}") } else { format!("Message @{title}") };
            self.composer.update(cx, |s, cx| {
                s.set_placeholder(ph, window, cx);
                s.focus(window, cx);
            });
        }
        cx.notify();
    }

    fn close_dm(&mut self, key: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(s) = self.store.as_mut() {
            if let Some(d) = s.me.dms.get_mut(key) {
                d.closed = true;
            }
            s.save();
        }
        if self.view == View::Dm(key.to_string()) {
            self.open_view(View::Friends(Tab::Online), window, cx);
        }
        cx.notify();
    }

    fn send_friend_request(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = self.add_input.read(cx).value().trim().trim_start_matches('@').to_lowercase();
        if input.is_empty() {
            return;
        }
        if crate::store::is_uid(&input) {
            let r = self.store.as_mut().map(|s| s.request_friend(&input, None));
            self.finish_request(r, window, cx);
            return;
        }
        let ok_name = input.len() >= 2 && input.len() <= 32 && input.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_' || b == b'.');
        if !ok_name {
            self.add_result = Some((false, "Hm, that doesn't look like a username or friend code.".into()));
            cx.notify();
            return;
        }
        self.add_result = Some((true, "Looking them up…".into()));
        cx.notify();
        let name = input.clone();
        let lookup = cx.background_executor().spawn(async move { net::lookup(SERVER, &name) });
        cx.spawn_in(window, async move |this, cx| {
            let res = lookup.await;
            let _ = this.update_in(cx, |this, window, cx| {
                let r = match res {
                    Err(_) => Some(Err("Couldn't check that right now. Try again in a moment.".to_string())),
                    Ok(None) => Some(Err("Hm, didn't work. Double check that the username is correct.".to_string())),
                    Ok(Some(uid)) => this.store.as_mut().map(|s| s.request_friend(&uid, Some(input.clone()))),
                };
                this.finish_request(r, window, cx);
            });
        })
        .detach();
    }

    fn finish_request(&mut self, r: Option<Result<String, String>>, window: &mut Window, cx: &mut Context<Self>) {
        match r {
            Some(Ok(t)) => {
                self.add_result = Some((true, t));
                self.add_input.update(cx, |s, cx| s.set_value("", window, cx));
            }
            Some(Err(t)) => self.add_result = Some((false, t)),
            None => {}
        }
        self.after_change(cx);
    }

    fn start_fresh(&mut self, cx: &mut Context<Self>) {
        let name: String = self.name_input.read(cx).value().split_whitespace().collect::<Vec<_>>().join(" ").chars().take(32).collect();
        if name.is_empty() {
            self.setup_error = Some("Type a name first.".into());
            cx.notify();
            return;
        }
        self.store = Some(Store::create(&name));
        self.started(cx);
        cx.notify();
    }

    fn link(&mut self, cx: &mut Context<Self>) {
        let code = self.link_input.read(cx).value().to_string();
        let name = self.name_input.read(cx).value().trim().to_string();
        match Store::link(&code, if name.is_empty() { "Someone" } else { &name }) {
            Ok(s) => {
                self.store = Some(s);
                self.setup_error = None;
                self.started(cx);
                self.toast("Linked! Your friends and DMs from the browser are here now.", 5000);
            }
            Err(e) => self.setup_error = Some(e),
        }
        cx.notify();
    }

    /// Logging out forgets this account on this computer (settings stay).
    pub(crate) fn log_out(&mut self, cx: &mut Context<Self>) {
        let _ = std::fs::remove_file(crate::store::data_dir().join("state.json"));
        self.store = None;
        self.net = None; // dropping the handle ends the connection thread
        self.settings = None;
        self.logout_armed = false;
        self.view = View::Friends(Tab::Online);
        cx.notify();
    }

    /* ---------- keyboard ---------- */

    fn zoom_in(&mut self, _: &ZoomIn, _: &mut Window, cx: &mut Context<Self>) {
        let z = zoom_step(self.prefs.zoom, 1);
        self.set_zoom(z, cx);
        self.toast(&format!("Zoom {z}%"), 1200);
    }
    fn zoom_out(&mut self, _: &ZoomOut, _: &mut Window, cx: &mut Context<Self>) {
        let z = zoom_step(self.prefs.zoom, -1);
        self.set_zoom(z, cx);
        self.toast(&format!("Zoom {z}%"), 1200);
    }
    fn zoom_reset(&mut self, _: &ZoomReset, _: &mut Window, cx: &mut Context<Self>) {
        self.set_zoom(100, cx);
        self.toast("Zoom 100%", 1200);
    }
    fn toggle_settings(&mut self, _: &ToggleSettings, window: &mut Window, cx: &mut Context<Self>) {
        if self.store.is_none() {
            return;
        }
        if self.settings.is_some() { self.close_settings(window, cx) } else { self.open_settings(Page::Account, window, cx) }
    }
    fn close_overlay(&mut self, _: &CloseOverlay, window: &mut Window, cx: &mut Context<Self>) {
        if self.status_menu {
            self.status_menu = false;
            cx.notify();
        } else if self.settings.is_some() {
            self.close_settings(window, cx);
        }
    }

    pub(crate) fn open_settings(&mut self, page: Page, window: &mut Window, cx: &mut Context<Self>) {
        self.settings = Some(page);
        self.status_menu = false;
        self.account_msg = None;
        self.logout_armed = false;
        if let Some(s) = &self.store {
            let (name, user) = (s.me.name.clone(), s.me.username.clone());
            self.rename_input.update(cx, |i, cx| i.set_value(name, window, cx));
            self.username_input.update(cx, |i, cx| i.set_value(user, window, cx));
        }
        window.focus(&self.focus, cx);
        cx.notify();
    }

    pub(crate) fn close_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.settings = None;
        if let View::Dm(k) = self.view.clone() {
            if let Some(s) = self.store.as_mut() {
                s.unread.remove(&k);
            }
            self.composer.update(cx, |s, cx| s.focus(window, cx));
        } else {
            window.focus(&self.focus, cx);
        }
        cx.notify();
    }

    /* ---------- drawing ---------- */

    fn render_setup(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let label = |t: &'static str| div().text_xs().font_weight(FontWeight::BOLD).text_color(c_muted()).child(t);
        div()
            .size_full()
            .bg(c_rail())
            .flex()
            .items_center()
            .justify_center()
            .child(
                div()
                    .w(u(480.))
                    .p(u(32.))
                    .rounded(u(8.))
                    .bg(c_main())
                    .shadow_lg()
                    .flex()
                    .flex_col()
                    .gap(u(10.))
                    .child(div().text_2xl().font_weight(FontWeight::BOLD).text_color(c_strong()).text_center().child("Welcome to LiteCord"))
                    .child(div().text_sm().text_color(c_muted()).text_center().mb(u(10.)).child("The native desktop app. No account needed."))
                    .child(label("DISPLAY NAME"))
                    .child(Input::new(&self.name_input))
                    .child(Button::new("fresh").primary().label("Start fresh").on_click(cx.listener(|this, _, _, cx| this.start_fresh(cx))))
                    .child(div().my(u(8.)).h(px(1.)).bg(c_border()))
                    .child(label("OR LINK YOUR BROWSER ACCOUNT"))
                    .child(div().text_sm().text_color(c_muted()).child(
                        "In LiteCord in your browser, open User Settings › Devices › Copy Link Code, and paste the code here. Your friends and DMs come along.",
                    ))
                    .child(Input::new(&self.link_input))
                    .child(Button::new("link").label("Link").on_click(cx.listener(|this, _, _, cx| this.link(cx))))
                    .children(self.setup_error.clone().map(|e| div().text_sm().text_color(c_red()).child(e)))
                    .child(div().mt(u(6.)).text_xs().text_color(c_muted()).text_center().child("Too small or too big? Ctrl + and Ctrl − change the size.")),
            )
            .into_any_element()
    }

    fn render_rail(&self, cx: &mut Context<Self>) -> Stateful<Div> {
        let unread: usize = self.store.as_ref().map(|s| s.unread.values().sum()).unwrap_or(0);
        let pending = self.store.as_ref().map(|s| s.me.friends.values().filter(|f| f.state == FriendState::In).count()).unwrap_or(0);
        let badge = unread + pending;
        let home = !matches!(self.view, View::Server(_));
        let servers = self.rail_servers(cx);
        div()
            .id("rail")
            .overflow_y_scroll()
            .w(u(72.))
            .flex_none()
            .h_full()
            .bg(c_rail())
            .flex()
            .flex_col()
            .items_center()
            .pt(u(12.))
            .gap(u(8.))
            .child(
                div()
                    .relative()
                    .w_full()
                    .flex()
                    .justify_center()
                    // The white pill on the left edge, like Discord's selected-server marker.
                    .when(home, |d| d.child(div().absolute().left_0().top(u(4.)).w(u(4.)).h(u(40.)).rounded_r(u(4.)).bg(c_strong())))
                    .child(
                        div()
                            .id("home")
                            .relative()
                            .size(u(48.))
                            .rounded(u(16.))
                            .bg(c_accent())
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_color(white())
                            .font_weight(FontWeight::BOLD)
                            .cursor_pointer()
                            .child("LC")
                            .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new("Direct Messages").build(window, cx))
                            .on_click(cx.listener(|this, _, window, cx| this.open_view(View::Friends(Tab::Online), window, cx)))
                            .when(badge > 0, |d| {
                                d.child(
                                    div()
                                        .absolute()
                                        .right(u(-4.))
                                        .bottom(u(-4.))
                                        .min_w(u(22.))
                                        .h(u(22.))
                                        .px(u(5.))
                                        .rounded_full()
                                        .bg(c_red())
                                        .border(u(4.))
                                        .border_color(c_rail())
                                        .text_color(white())
                                        .text_size(u(11.))
                                        .font_weight(FontWeight::BOLD)
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .child(badge.to_string()),
                                )
                            }),
                    ),
            )
            .child(div().w(u(32.)).h(u(2.)).rounded_full().bg(c_active()))
            .children(servers)
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> Div {
        let server_side = match &self.view {
            View::Server(r) => Some(self.server_sidebar(r, cx)),
            _ => None,
        };
        let store = self.store.as_ref().unwrap();
        let friends_on = matches!(self.view, View::Friends(_));
        let pending = store.me.friends.values().filter(|f| f.state == FriendState::In).count();
        let mut list = div().id("dmlist").flex_1().min_h(px(0.)).overflow_y_scroll().px(u(8.)).flex().flex_col().gap(u(2.));
        list = list.child(
            div()
                .id("friends")
                .h(u(42.))
                .px(u(12.))
                .rounded(u(6.))
                .flex()
                .items_center()
                .gap(u(12.))
                .cursor_pointer()
                .text_color(if friends_on { c_strong() } else { c_muted() })
                .when(friends_on, |d| d.bg(c_active()))
                .hover(|d| d.bg(c_hover()).text_color(c_text()))
                .child(Icon::new(IconName::User).size(u(20.)))
                .child(div().flex_1().font_weight(FontWeight::MEDIUM).child("Friends"))
                .when(pending > 0, |d| d.child(count_badge(pending)))
                .on_click(cx.listener(|this, _, window, cx| this.open_view(View::Friends(Tab::Online), window, cx))),
        );
        list = list.child(div().mt(u(12.)).mb(u(4.)).px(u(8.)).text_xs().font_weight(FontWeight::SEMIBOLD).text_color(c_muted()).child("Direct Messages"));
        let dm_list = store.dm_list();
        for key in &dm_list {
            let on = self.view == View::Dm(key.clone());
            let title = store.dm_title(key);
            let unread = store.unread.get(key).copied().unwrap_or(0);
            let (icon, sub) = match store.dm_other(key) {
                Some(uid) => (avatar_dot(&uid, &title, 32., store.status_of(&uid), if on { c_active() } else { c_side() }), store.subline(&uid)),
                None => {
                    let n = store.me.dms.get(key).map(|d| d.members.len()).unwrap_or(0);
                    (avatar(key, &title, 32.), format!("{n} Members"))
                }
            };
            let (k, k2) = (key.clone(), key.clone());
            list = list.child(
                div()
                    .id(SharedString::from(format!("dm-{key}")))
                    .group("dmrow")
                    .h(u(42.))
                    .px(u(8.))
                    .rounded(u(6.))
                    .flex()
                    .items_center()
                    .gap(u(12.))
                    .cursor_pointer()
                    .when(on, |d| d.bg(c_active()))
                    .hover(|d| d.bg(c_hover()))
                    .child(icon)
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .truncate()
                                    .text_color(if on || unread > 0 { c_strong() } else { c_muted() })
                                    .font_weight(if unread > 0 { FontWeight::SEMIBOLD } else { FontWeight::MEDIUM })
                                    .child(title),
                            )
                            .when(!sub.is_empty(), |d| d.child(div().truncate().text_xs().text_color(c_muted()).child(sub))),
                    )
                    .when(unread > 0, |d| d.child(count_badge(unread)))
                    // The little X that shows on hover, to close a DM (it comes back with the next message).
                    .child(
                        div()
                            .id(SharedString::from(format!("x-{key}")))
                            .invisible()
                            .group_hover("dmrow", |s| s.visible())
                            .text_color(c_muted())
                            .hover(|d| d.text_color(c_strong()))
                            .child(Icon::new(IconName::Close).size(u(16.)))
                            .on_click(cx.listener(move |this, _, window, cx| {
                                cx.stop_propagation();
                                this.close_dm(&k2, window, cx);
                            })),
                    )
                    .on_click(cx.listener(move |this, _, window, cx| this.open_view(View::Dm(k.clone()), window, cx))),
            );
        }
        if dm_list.is_empty() {
            list = list.child(div().px(u(8.)).text_sm().text_color(c_muted()).child("Your DMs show up here. Message a friend to start one."));
        }
        let shown = if !store.online { "offline" } else { self.prefs.status.as_str() };
        let line = if !store.online {
            "Connecting…".to_string()
        } else if !self.prefs.custom.is_empty() {
            self.prefs.custom.clone()
        } else {
            status_label(shown).to_string()
        };
        div()
            .w(u(240.))
            .flex_none()
            .h_full()
            .bg(c_side())
            .flex()
            .flex_col()
            .when_some(server_side.as_ref().map(|_| ()), |d, _| d)
            .when(server_side.is_none(), |d| d.child(
                div().h(u(48.)).flex_none().px(u(10.)).flex().items_center().border_b_1().border_color(c_border()).child(
                    div()
                        .id("find")
                        .w_full()
                        .h(u(28.))
                        .px(u(8.))
                        .rounded(u(6.))
                        .bg(c_rail())
                        .flex()
                        .items_center()
                        .text_sm()
                        .text_color(c_muted())
                        .cursor_pointer()
                        .child("Find or start a conversation")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.open_view(View::Friends(Tab::Add), window, cx);
                            this.add_input.update(cx, |s, cx| s.focus(window, cx));
                        })),
                ),
            ))
            .map(|d| match server_side {
                Some((head, chans)) => d.child(head).child(chans),
                None => d.child(div().h(u(8.))).child(list),
            })
            // The user bar, floating like Discord's.
            .child(
                div().p(u(8.)).flex_none().child(
                    div().rounded(u(8.)).bg(c_panel()).flex().flex_col().children(self.render_voice_panel(cx)).child(
                    div()
                        .h(u(52.))
                        .px(u(6.))
                        .rounded(u(8.))
                        .bg(c_panel())
                        .flex()
                        .items_center()
                        .gap(u(4.))
                        .child(
                            div()
                                .id("me")
                                .flex_1()
                                .min_w(px(0.))
                                .h(u(42.))
                                .px(u(4.))
                                .rounded(u(6.))
                                .flex()
                                .items_center()
                                .gap(u(8.))
                                .cursor_pointer()
                                .hover(|d| d.bg(c_hover()))
                                .child(avatar_dot(&store.me.uid, &store.me.name, 32., shown, c_panel()))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w(px(0.))
                                        .flex()
                                        .flex_col()
                                        .child(div().truncate().text_sm().font_weight(FontWeight::SEMIBOLD).text_color(c_strong()).child(store.me.name.clone()))
                                        .child(div().truncate().text_xs().text_color(c_muted()).child(line)),
                                )
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.status_menu = !this.status_menu;
                                    cx.notify();
                                })),
                        )
                        .children(self.user_bar_buttons(cx))
                        .child(
                            Button::new("opensettings")
                                .ghost()
                                .compact()
                                .icon(IconName::Settings)
                                .tooltip("User Settings")
                                .on_click(cx.listener(|this, _, window, cx| this.open_settings(Page::Account, window, cx))),
                        ),
                    ),
                ),
            )
    }

    /// The menu that opens from your name: pick a status, or set a custom one.
    fn render_status_menu(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let opt = |id: &'static str, status: &'static str, desc: &'static str, cx: &mut Context<Self>| {
            let on = self.prefs.status == status;
            div()
                .id(id)
                .px(u(8.))
                .py(u(6.))
                .rounded(u(4.))
                .flex()
                .gap(u(10.))
                .cursor_pointer()
                .hover(|d| d.bg(c_accent()).text_color(white()))
                .child(div().mt(u(5.)).size(u(10.)).rounded_full().bg(status_color(status)).flex_none())
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .child(div().text_sm().font_weight(FontWeight::MEDIUM).child(status_label(status)))
                        .when(!desc.is_empty(), |d| d.child(div().text_xs().opacity(0.8).child(desc))),
                )
                .when(on, |d| d.child(Icon::new(IconName::Check).size(u(16.))))
                .on_click(cx.listener(move |this, _, _, cx| {
                    let custom = this.prefs.custom.clone();
                    this.set_status(status, &custom, cx);
                    this.status_menu = false;
                }))
        };
        div()
            .id("statusmenu")
            .absolute()
            .left(u(80.))
            .bottom(u(70.))
            .w(u(300.))
            .p(u(8.))
            .rounded(u(8.))
            .bg(c_float())
            .border_1()
            .border_color(c_border())
            .shadow_lg()
            .text_color(c_text())
            .flex()
            .flex_col()
            .gap(u(2.))
            .on_click(|_, _, cx| cx.stop_propagation())
            .child(opt("st-on", "online", "", cx))
            .child(opt("st-idle", "idle", "", cx))
            .child(opt("st-dnd", "dnd", "You won't get desktop notifications", cx))
            .child(opt("st-inv", "invisible", "You'll look offline, but still have full access", cx))
            .child(div().my(u(4.)).h(px(1.)).bg(c_border()))
            .child(div().px(u(8.)).pt(u(2.)).text_xs().font_weight(FontWeight::BOLD).text_color(c_muted()).child("CUSTOM STATUS"))
            .child(
                div()
                    .px(u(4.))
                    .flex()
                    .gap(u(6.))
                    .items_center()
                    .child(div().flex_1().child(Input::new(&self.custom_input)))
                    .child(Button::new("clearcustom").ghost().compact().icon(IconName::Close).tooltip("Clear Status").on_click(cx.listener(|this, _, window, cx| {
                        this.custom_input.update(cx, |s, cx| s.set_value("", window, cx));
                        let status = this.prefs.status.clone();
                        this.set_status(&status, "", cx);
                    }))),
            )
            .child(div().px(u(8.)).text_xs().text_color(c_muted()).child("Press Enter to save."))
    }

    pub(crate) fn header(&self, title: impl IntoElement) -> Div {
        div().h(u(48.)).flex_none().px(u(16.)).flex().items_center().gap(u(12.)).border_b_1().border_color(c_border()).child(title)
    }

    fn render_friends(&mut self, tab: Tab, cx: &mut Context<Self>) -> Div {
        let store = self.store.as_ref().unwrap();
        let pending = store.me.friends.values().filter(|f| f.state == FriendState::In).count();
        let tab_btn = |id: &'static str, label: &'static str, t: Tab, cx: &mut Context<Self>| {
            let on = tab == t;
            div()
                .id(id)
                .px(u(10.))
                .py(u(2.))
                .rounded(u(6.))
                .cursor_pointer()
                .font_weight(FontWeight::MEDIUM)
                .text_color(if on { c_strong() } else { c_muted() })
                .when(on, |d| d.bg(c_active()))
                .hover(|d| d.bg(c_hover()).text_color(c_text()))
                .flex()
                .items_center()
                .gap(u(6.))
                .child(label)
                .when(t == Tab::Pending && pending > 0, |d| d.child(count_badge(pending)))
                .on_click(cx.listener(move |this, _, window, cx| this.open_view(View::Friends(t), window, cx)))
        };
        let head = self.header(
            div()
                .flex()
                .items_center()
                .gap(u(8.))
                .child(Icon::new(IconName::User).size(u(20.)).text_color(c_muted()))
                .child(div().font_weight(FontWeight::SEMIBOLD).text_color(c_strong()).child("Friends"))
                .child(div().mx(u(8.)).size(u(4.)).rounded_full().bg(c_muted()))
                .child(tab_btn("t-on", "Online", Tab::Online, cx))
                .child(tab_btn("t-all", "All", Tab::All, cx))
                .child(tab_btn("t-pend", "Pending", Tab::Pending, cx))
                .child(tab_btn("t-blk", "Blocked", Tab::Blocked, cx))
                .child(
                    div()
                        .id("t-add")
                        .px(u(10.))
                        .py(u(2.))
                        .rounded(u(6.))
                        .cursor_pointer()
                        .font_weight(FontWeight::MEDIUM)
                        .when(tab == Tab::Add, |d| d.text_color(c_green()).bg(c_green().opacity(0.12)))
                        .when(tab != Tab::Add, |d| d.text_color(white()).bg(c_green()))
                        .child("Add Friend")
                        .on_click(cx.listener(|this, _, window, cx| {
                            this.open_view(View::Friends(Tab::Add), window, cx);
                            this.add_input.update(cx, |s, cx| s.focus(window, cx));
                        })),
                ),
        );
        let body = if tab == Tab::Add {
            let result = self.add_result.clone();
            div()
                .flex_1()
                .p(u(20.))
                .flex()
                .flex_col()
                .gap(u(8.))
                .child(div().text_lg().font_weight(FontWeight::BOLD).text_color(c_strong()).child("Add Friend"))
                .child(div().text_sm().text_color(c_muted()).child("You can add friends with their LiteCord username or friend code."))
                .child(
                    div()
                        .flex()
                        .gap(u(8.))
                        .items_center()
                        .child(div().flex_1().child(Input::new(&self.add_input)))
                        .child(Button::new("sendreq").primary().label("Send Friend Request").on_click(cx.listener(|this, _, window, cx| this.send_friend_request(window, cx)))),
                )
                .children(result.map(|(ok, t)| div().text_sm().text_color(if ok { c_green() } else { c_red() }).child(t)))
                .child(div().mt(u(16.)).text_sm().text_color(c_muted()).child(if store.me.username.is_empty() {
                    format!("Your friend code: {}  ·  Claim a username in Settings › My Account", store.me.uid)
                } else {
                    format!("Your username: {}  ·  Friend code: {}", store.me.username, store.me.uid)
                }))
        } else {
            let mut people: Vec<(String, String, FriendState)> = store
                .me
                .friends
                .iter()
                .filter(|(u, f)| match tab {
                    Tab::Online => f.state == FriendState::Friend && store.status_of(u) != "offline",
                    Tab::All => f.state == FriendState::Friend,
                    Tab::Pending => matches!(f.state, FriendState::In | FriendState::Out),
                    Tab::Blocked => f.state == FriendState::Blocked,
                    Tab::Add => false,
                })
                .map(|(u, f)| {
                    let n = store.name_of(u);
                    (u.clone(), if n == "Someone" { f.name.clone() } else { n }, f.state)
                })
                .collect();
            people.sort_by_key(|(_, n, _)| n.to_lowercase());
            let title = match tab {
                Tab::Online => "Online",
                Tab::All => "All Friends",
                Tab::Pending => "Pending",
                _ => "Blocked",
            };
            let mut list = div().id("friendlist").flex_1().min_h(px(0.)).overflow_y_scroll().px(u(20.)).flex().flex_col();
            list = list.child(div().pt(u(16.)).pb(u(8.)).text_xs().font_weight(FontWeight::SEMIBOLD).text_color(c_muted()).child(format!("{title} — {}", people.len())));
            if people.is_empty() {
                let empty = match tab {
                    Tab::Online => "No one's online right now. Your friends show up here when they are.",
                    Tab::Pending => "There are no pending friend requests.",
                    Tab::Blocked => "You haven't blocked anyone.",
                    _ => "You don't have any friends yet. Add some with Add Friend!",
                };
                list = list.child(div().pt(u(60.)).text_center().text_color(c_muted()).child(empty));
            }
            for (uid, name, state) in people {
                let status = store.status_of(&uid).to_string();
                let sub = match state {
                    FriendState::In => "Incoming Friend Request".to_string(),
                    FriendState::Out => "Outgoing Friend Request".to_string(),
                    FriendState::Blocked => "Blocked".to_string(),
                    FriendState::Friend => {
                        let s = store.subline(&uid);
                        if s.is_empty() { status_label(&status).to_string() } else { s }
                    }
                };
                let mut actions = div().flex().gap(u(8.));
                let u_ = uid.clone();
                match state {
                    FriendState::Friend => {
                        let u2 = uid.clone();
                        actions = actions
                            .child(round_btn(SharedString::from(format!("msg-{uid}")), IconName::Inbox, "Message").on_click(cx.listener(move |this, _, window, cx| {
                                let key = this.store.as_mut().map(|s| s.dm_for(&u_));
                                if let Some(k) = key {
                                    this.open_view(View::Dm(k), window, cx);
                                }
                            })))
                            .child(round_btn(SharedString::from(format!("rm-{uid}")), IconName::Close, "Remove Friend").on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(s) = this.store.as_mut() {
                                    s.remove_friend(&u2);
                                }
                                this.after_change(cx);
                            })));
                    }
                    FriendState::In => {
                        let u2 = uid.clone();
                        actions = actions
                            .child(round_btn(SharedString::from(format!("ok-{uid}")), IconName::Check, "Accept").on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(s) = this.store.as_mut() {
                                    s.accept_friend(&u_);
                                }
                                this.after_change(cx);
                            })))
                            .child(round_btn(SharedString::from(format!("no-{uid}")), IconName::Close, "Ignore").on_click(cx.listener(move |this, _, _, cx| {
                                if let Some(s) = this.store.as_mut() {
                                    s.remove_friend(&u2);
                                }
                                this.after_change(cx);
                            })));
                    }
                    _ => {
                        actions = actions.child(
                            round_btn(SharedString::from(format!("rm-{uid}")), IconName::Close, if state == FriendState::Out { "Cancel" } else { "Unblock" }).on_click(cx.listener(
                                move |this, _, _, cx| {
                                    if let Some(s) = this.store.as_mut() {
                                        s.remove_friend(&u_);
                                    }
                                    this.after_change(cx);
                                },
                            )),
                        );
                    }
                }
                list = list.child(
                    div()
                        .id(SharedString::from(format!("f-{uid}")))
                        .h(u(62.))
                        .px(u(10.))
                        .border_t_1()
                        .border_color(c_border())
                        .rounded(u(8.))
                        .flex()
                        .items_center()
                        .gap(u(12.))
                        .hover(|d| d.bg(c_hover()))
                        .child(avatar_dot(&uid, &name, 32., &status, c_main()))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .flex()
                                .flex_col()
                                .child(div().font_weight(FontWeight::SEMIBOLD).text_color(c_strong()).child(name))
                                .child(div().text_sm().text_color(c_muted()).truncate().child(sub)),
                        )
                        .child(actions),
                );
            }
            div().flex_1().min_h(px(0.)).flex().flex_col().child(list)
        };
        div().flex_1().min_w(px(0.)).h_full().bg(c_main()).flex().flex_col().child(head).child(body)
    }

    fn render_dm(&mut self, key: String, cx: &mut Context<Self>) -> Div {
        let call_buttons = self.dm_call_buttons(&key, cx);
        // The call area sits above the chat while this DM's call room is open (like the web app).
        let call_area = if self.call.dm.as_deref() == Some(key.as_str()) {
            Some(self.render_call_area(crate::call::DM_VOICE, Some(&key), Some(440.), cx))
        } else {
            None
        };
        let store = self.store.as_ref().unwrap();
        let title = store.dm_title(&key);
        let other = store.dm_other(&key);
        let typing = store.typing_in(&key);
        let rows = self.rows.clone();
        let my_uid = store.me.uid.clone();
        let names: Rc<std::collections::HashMap<String, String>> = Rc::new(
            rows.iter()
                .map(|r| r.msg.uid.clone())
                .filter(|u| !u.is_empty())
                .map(|u| {
                    let n = store.name_of(&u);
                    (u, n)
                })
                .collect(),
        );
        let intro_title = title.clone();
        let intro_uid = other.clone().unwrap_or_else(|| key.clone());
        let is_group = other.is_none();
        let compact = self.prefs.compact;
        let font = self.prefs.chat_font as f32;
        let h24 = self.prefs.clock24();
        let me = cx.entity().downgrade();
        let dm_key = key.clone();
        let head = self.header(
            div()
                .flex_1()
                .flex()
                .items_center()
                .gap(u(10.))
                .child(match &other {
                    Some(id) => avatar_dot(id, &title, 24., store.status_of(id), c_main()),
                    None => avatar(&key, &title, 24.),
                })
                .child(div().font_weight(FontWeight::SEMIBOLD).text_color(c_strong()).child(title.clone()))
                .children(other.as_ref().map(|id| {
                    let sub = store.subline(id);
                    div().text_sm().text_color(c_muted()).child(if sub.is_empty() { status_label(store.status_of(id)).to_string() } else { sub })
                }))
                .child(div().flex_1())
                .children(call_buttons),
        );
        let row_count = rows.len();
        let scroller = MessageScroller::new("msgs", self.scroller.clone(), move |ix, _, _| {
            let r = &rows[ix];
            let m = &r.msg;
            let mut out = div().flex().flex_col();
            if ix == 0 {
                out = out.child(dm_intro(&intro_uid, &intro_title, is_group));
            }
            if let Some(day) = &r.day {
                out = out.child(
                    div()
                        .mx(u(16.))
                        .my(u(8.))
                        .flex()
                        .items_center()
                        .gap(u(8.))
                        .child(div().flex_1().h(px(1.)).bg(c_border()))
                        .child(div().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(c_muted()).child(day.clone()))
                        .child(div().flex_1().h(px(1.)).bg(c_border())),
                );
            }
            if m.sys {
                return out.child(
                    div()
                        .px(u(16.))
                        .py(u(4.))
                        .flex()
                        .gap(u(8.))
                        .text_size(u(font - 1.))
                        .text_color(c_muted())
                        .child(div().text_color(c_green()).child("→"))
                        .child(m.text.clone())
                        .child(div().text_xs().child(when(m.ts, h24))),
                );
            }
            let name = if m.uid == my_uid { m.name.clone() } else { names.get(&m.uid).cloned().unwrap_or_else(|| m.name.clone()) };
            let name_color = hsla(hue(&m.uid) / 360., 0.6, if pal().light { 0.35 } else { 0.75 }, 1.);
            let mut body = div().text_size(u(font)).line_height(u(font * 1.375)).text_color(c_text()).child(m.text.clone());
            if let Some(f) = &m.file {
                body = body.child(div().text_sm().text_color(c_muted()).child(format!("📎 {f} (open it in the browser for now)")));
            }
            if m.edited.is_some() {
                body = body.child(div().text_xs().text_color(c_muted()).child("(edited)"));
            }
            // The hover toolbar in the top-right corner, like Discord's.
            let mid = m.mid.clone();
            let text = m.text.clone();
            let own = m.uid == my_uid;
            let (me1, me2, k1) = (me.clone(), me.clone(), dm_key.clone());
            let tools = div()
                .absolute()
                .top(u(-14.))
                .right(u(16.))
                .invisible()
                .group_hover("msg", |s| s.visible())
                .flex()
                .rounded(u(6.))
                .bg(c_main())
                .border_1()
                .border_color(c_border())
                .shadow_sm()
                .child(tool_btn(SharedString::from(format!("cp-{mid}")), IconName::Copy).on_click(move |_, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(text.clone()));
                    let _ = me1.update(cx, |this, cx| {
                        this.toast("Copied!", 1500);
                        cx.notify();
                    });
                }))
                .when(own, |d| {
                    let mid2 = mid.clone();
                    d.child(tool_btn(SharedString::from(format!("del-{mid}")), IconName::Delete).text_color(c_red()).on_click(move |_, _, cx| {
                        let _ = me2.update(cx, |this, cx| {
                            if let Some(s) = this.store.as_mut() {
                                s.delete_msg(&k1, &mid2);
                            }
                            this.after_change(cx);
                        });
                    }))
                });
            let line = div().relative().group("msg").px(u(16.)).pr(u(48.)).hover(|d| d.bg(c_msghover())).flex().gap(u(16.)).child(tools);
            out.child(if compact {
                // One line per message: time, name, text.
                line.py(u(1.))
                    .gap(u(8.))
                    .child(div().flex_none().w(u(if h24 { 40. } else { 64. })).text_xs().text_color(c_muted()).pt(u(3.)).child(clock(m.ts, h24)))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .flex()
                            .gap(u(6.))
                            .child(div().flex_none().text_size(u(font)).font_weight(FontWeight::MEDIUM).text_color(name_color).child(name))
                            .child(body),
                    )
            } else if r.head {
                line.mt(u(16.))
                    .py(u(2.))
                    .child(avatar(&m.uid, &name, 40.))
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .flex()
                                    .items_baseline()
                                    .gap(u(8.))
                                    .child(div().text_size(u(font)).font_weight(FontWeight::MEDIUM).text_color(name_color).child(name))
                                    .child(div().text_xs().text_color(c_muted()).child(when(m.ts, h24))),
                            )
                            .child(body),
                    )
            } else {
                line.py(u(1.))
                    .child(div().w(u(40.)).flex_none().invisible().group_hover("msg", |s| s.visible()).text_size(u(10.)).text_color(c_muted()).pt(u(4.)).child(clock(m.ts, h24)))
                    .child(div().flex_1().min_w(px(0.)).child(body))
            })
        })
        .scrollbar(true)
        .with_bottom_fade(c_main())
        .with_row_style(StyleRefinement::default().pb_0())
        .with_list_style(StyleRefinement::default().pb(u(12.)))
        .flex_1()
        .min_h(px(0.));
        let typing_line = match typing.len() {
            0 => String::new(),
            1 => format!("{} is typing…", typing[0]),
            2 => format!("{} and {} are typing…", typing[0], typing[1]),
            _ => "Several people are typing…".into(),
        };
        div()
            .flex_1()
            .min_w(px(0.))
            .h_full()
            .bg(c_main())
            .flex()
            .flex_col()
            .child(head)
            .children(call_area)
            .child(if row_count == 0 {
                div().flex_1().flex().flex_col().justify_end().child(dm_intro(other.as_deref().unwrap_or(&key), &title, other.is_none())).into_any_element()
            } else {
                scroller.into_any_element()
            })
            .child(
                div().px(u(16.)).flex_none().child(
                    div().px(u(12.)).py(u(4.)).rounded(u(8.)).bg(c_input()).text_size(u(font)).child(Input::new(&self.composer).appearance(false)),
                ),
            )
            .child(div().h(u(24.)).px(u(16.)).flex_none().flex().items_center().text_xs().text_color(c_text()).child(typing_line))
    }
}

/// Discord's "This is the beginning of your direct message history with …" header.
fn dm_intro(uid: &str, title: &str, group: bool) -> Div {
    div()
        .px(u(16.))
        .pt(u(16.))
        .pb(u(8.))
        .flex()
        .flex_col()
        .gap(u(8.))
        .child(avatar(uid, title, 80.))
        .child(div().text_3xl().font_weight(FontWeight::BOLD).text_color(c_strong()).child(title.to_string()))
        .child(div().text_color(c_muted()).child(if group {
            format!("Welcome to the beginning of the {title} group.")
        } else {
            format!("This is the beginning of your direct message history with {title}.")
        }))
}

pub fn count_badge(n: usize) -> Div {
    div()
        .min_w(u(16.))
        .h(u(16.))
        .px(u(4.))
        .rounded_full()
        .bg(c_red())
        .text_color(white())
        .text_size(u(11.))
        .font_weight(FontWeight::BOLD)
        .flex()
        .items_center()
        .justify_center()
        .child(n.to_string())
}

fn round_btn(id: SharedString, icon: IconName, tip: &'static str) -> Button {
    Button::new(id).ghost().icon(icon).tooltip(tip).rounded(ButtonRounded::Large)
}

fn tool_btn(id: SharedString, icon: IconName) -> Stateful<Div> {
    div()
        .id(id)
        .size(u(30.))
        .flex()
        .items_center()
        .justify_center()
        .text_color(c_muted())
        .cursor_pointer()
        .hover(|d| d.bg(c_hover()).text_color(c_strong()))
        .child(Icon::new(icon).size(u(16.)))
}

impl Render for LiteCord {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Everything made of rems (text sizes, the input boxes and buttons) follows the UI scale too.
        // Only when they change: setting them asks for another frame, which would loop forever.
        if window.rem_size() != u(16.) {
            window.set_rem_size(u(16.));
        }
        let unread: usize = self.store.as_ref().map(|s| s.unread.values().sum()).unwrap_or(0);
        let title = if unread > 0 { format!("({unread}) LiteCord") } else { "LiteCord".to_string() };
        if title != self.window_title {
            window.set_window_title(&title);
            self.window_title = title;
        }

        for old in self.old_images.drain(..) {
            let _ = window.drop_image(old);
        }
        let title = self.title_text();
        let content: AnyElement = if self.store.is_none() {
            self.render_setup(cx)
        } else if let Some(page) = self.settings {
            self.render_settings(page, window, cx).into_any_element()
        } else {
            let main = match self.view.clone() {
                View::Friends(t) => self.render_friends(t, cx),
                View::Dm(k) => self.render_dm(k, cx),
                View::Server(r) => self.render_server(r, cx),
            };
            div().size_full().flex().child(self.render_rail(cx)).child(self.render_sidebar(cx)).child(main).into_any_element()
        };
        let toast = self.toast.clone();
        let app = div()
            .id("app")
            .track_focus(&self.focus)
            .key_context("LiteCord")
            .on_action(cx.listener(Self::zoom_in))
            .on_action(cx.listener(Self::zoom_out))
            .on_action(cx.listener(Self::zoom_reset))
            .on_action(cx.listener(Self::toggle_settings))
            .on_action(cx.listener(Self::close_overlay))
            // Push to talk (and picking its key) sees every key and side mouse button first.
            .capture_key_down(cx.listener(|this, e: &KeyDownEvent, _, cx| {
                if !e.is_held && this.voice_key(&e.keystroke.key, true, cx) {
                    cx.stop_propagation();
                }
            }))
            .capture_key_up(cx.listener(|this, e: &KeyUpEvent, _, cx| {
                this.voice_key(&e.keystroke.key, false, cx);
            }))
            .capture_any_mouse_down(cx.listener(|this, e: &MouseDownEvent, _, cx| {
                if let MouseButton::Navigate(n) = e.button {
                    let k = if n == NavigationDirection::Back { "mouse4" } else { "mouse5" };
                    if this.voice_key(k, true, cx) {
                        cx.stop_propagation();
                    }
                }
            }))
            .capture_any_mouse_up(cx.listener(|this, e: &MouseUpEvent, _, cx| {
                if let MouseButton::Navigate(n) = e.button {
                    this.voice_key(if n == NavigationDirection::Back { "mouse4" } else { "mouse5" }, false, cx);
                }
            }))
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .bg(c_rail())
            .text_color(c_text())
            .text_size(u(15.))
            .child(self.render_title_bar(&title, window, cx))
            .child(div().flex_1().min_h(px(0.)).relative().child(content))
            // With the system's own title bar, the update button sits at the top right of the window.
            .when(!self.custom_frame(window), |d| {
                d.children(self.render_update_button(cx).map(|b| div().absolute().top(u(10.)).right(u(12.)).child(b)))
            })
            .when(self.status_menu && self.store.is_some() && self.settings.is_none(), |d| {
                d.child(
                    div()
                        .id("backdrop")
                        .absolute()
                        .size_full()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.status_menu = false;
                            cx.notify();
                        })),
                )
                .child(self.render_status_menu(cx))
            })
            .when(self.server.adding && self.settings.is_none(), |d| {
                d.child(
                    div().id("addbackdrop").absolute().size_full().on_click(cx.listener(|this, _, _, cx| {
                        this.server.adding = false;
                        cx.notify();
                    }))
                    .child(self.render_add_server(cx)),
                )
            })
            .when(self.settings.is_none(), |d| d.children(self.render_ring_card(cx)))
            .when(self.settings.is_none(), |d| d.children(self.render_user_menu(cx)))
            .children(toast.map(|(t, _)| {
                div().absolute().bottom(u(80.)).left_0().right_0().flex().justify_center().child(
                    div()
                        .max_w(u(520.))
                        .px(u(16.))
                        .py(u(10.))
                        .rounded(u(8.))
                        .bg(c_float())
                        .border_1()
                        .border_color(c_border())
                        .shadow_lg()
                        .text_color(c_strong())
                        .text_sm()
                        .child(t),
                )
            }));
        // On Linux with LiteCord's own frame, this adds the soft shadow and the edges you drag to resize.
        // On Linux with LiteCord's own frame: a thin edge you can drag to resize (no shadow band,
        // which KDE drew as a thick dark frame).
        window_border().shadow_size(px(0.)).child(app)
    }
}

//! The window: server rail, sidebar (Friends + Direct Messages + your user bar) and the main area
//! (the Friends page or a DM). Colors are the web app's "Dark" theme tokens.

use std::rc::Rc;
use std::time::Duration;

use chrono::{Local, TimeZone};
use gpui_kit::component::button::*;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::message_scroller::{MessageScroller, MessageScrollerState};
use gpui_kit::component::*;
use gpui_kit::*;
use gpui_kit::{prelude::FluentBuilder as _, white};

use crate::net::{self, NetEvent, NetHandle};
use crate::store::{now_ms, FriendState, Msg, Notice, Store};

pub const SERVER: &str = "https://litecord.litecord.workers.dev";

/* ---------- colors (cloudflare/public/index.html, [data-theme=dark]) ---------- */
fn c_rail() -> Hsla { rgb(0x121214).into() }
fn c_side() -> Hsla { rgb(0x18181b).into() }
fn c_main() -> Hsla { rgb(0x1d1e21).into() }
fn c_panel() -> Hsla { rgb(0x141416).into() }
fn c_input() -> Hsla { rgb(0x25262a).into() }
fn c_hover() -> Hsla { rgb(0x26272b).into() }
fn c_active() -> Hsla { rgb(0x303136).into() }
fn c_msghover() -> Hsla { rgb(0x212226).into() }
fn c_border() -> Hsla { rgb(0x2a2b30).into() }
fn c_text() -> Hsla { rgb(0xdbdee1).into() }
fn c_strong() -> Hsla { rgb(0xf2f3f5).into() }
fn c_muted() -> Hsla { rgb(0x949ba4).into() }
fn c_accent() -> Hsla { rgb(0x5865f2).into() }
fn c_green() -> Hsla { rgb(0x23a55a).into() }
fn c_red() -> Hsla { rgb(0xda373c).into() }
fn c_yellow() -> Hsla { rgb(0xf0b232).into() }

/// The web app's hue(): the same person gets the same avatar color everywhere.
fn hue(s: &str) -> f32 {
    let mut n: u32 = 0;
    for c in s.encode_utf16() {
        n = (n * 31 + c as u32) % 360;
    }
    n as f32
}

fn status_color(s: &str) -> Hsla {
    match s {
        "online" => c_green(),
        "idle" => c_yellow(),
        "dnd" => c_red(),
        _ => rgb(0x80848e).into(),
    }
}

fn status_label(s: &str) -> &'static str {
    match s {
        "online" => "Online",
        "idle" => "Idle",
        "dnd" => "Do Not Disturb",
        _ => "Offline",
    }
}

fn avatar(uid: &str, name: &str, size: f32) -> Div {
    let letter: String = name.chars().next().unwrap_or('?').to_uppercase().collect();
    div()
        .flex_none()
        .size(px(size))
        .rounded_full()
        .bg(hsla(hue(uid) / 360., 0.45, 0.40, 1.))
        .flex()
        .items_center()
        .justify_center()
        .text_color(white())
        .text_size(px(size * 0.42))
        .font_weight(FontWeight::SEMIBOLD)
        .child(letter)
}

/// An avatar with the little status dot in the corner, cut out of the background like Discord's.
fn avatar_dot(uid: &str, name: &str, size: f32, status: &str, ring: Hsla) -> Div {
    let dot = size * 0.32;
    div().relative().flex_none().size(px(size)).child(avatar(uid, name, size)).child(
        div()
            .absolute()
            .right(px(-2.))
            .bottom(px(-2.))
            .size(px(dot + 5.))
            .rounded_full()
            .bg(ring)
            .flex()
            .items_center()
            .justify_center()
            .child(div().size(px(dot)).rounded_full().bg(status_color(status))),
    )
}

fn when(ts: i64) -> String {
    let Some(t) = Local.timestamp_millis_opt(ts).single() else { return String::new() };
    let today = Local::now().date_naive();
    let day = t.date_naive();
    let clock = t.format("%-I:%M %p").to_string();
    if day == today {
        format!("Today at {clock}")
    } else if Some(day) == today.pred_opt() {
        format!("Yesterday at {clock}")
    } else {
        format!("{} {clock}", t.format("%m/%d/%Y"))
    }
}

#[derive(Clone, PartialEq)]
enum View {
    Friends(Tab),
    Dm(String),
}

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Online,
    All,
    Pending,
    Blocked,
    Add,
}

/// One line in the chat, worked out once per change so the list can draw rows quickly.
struct Row {
    msg: Msg,
    head: bool,   // first of a group: show avatar and name
    day: Option<String>, // a date divider above this row
}

pub struct LiteCord {
    store: Option<Store>,
    net: Option<NetHandle>,
    view: View,
    // setup screen
    name_input: Entity<InputState>,
    link_input: Entity<InputState>,
    setup_error: Option<String>,
    // main window
    composer: Entity<InputState>,
    add_input: Entity<InputState>,
    add_result: Option<(bool, String)>,
    scroller: Entity<MessageScrollerState>,
    rows: Rc<Vec<Row>>,
    toast: Option<(String, i64)>,
    last_typing: i64,
    _subs: Vec<Subscription>,
}

impl LiteCord {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let name_input = cx.new(|cx| InputState::new(window, cx).placeholder("What should everyone call you?"));
        let link_input = cx.new(|cx| InputState::new(window, cx).placeholder("lc1:…"));
        let composer = cx.new(|cx| InputState::new(window, cx).placeholder("Message"));
        let add_input = cx.new(|cx| InputState::new(window, cx).placeholder("You can add friends with their LiteCord username or friend code."));
        let scroller = cx.new(|cx| MessageScrollerState::new(0, cx));

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

        let mut app = LiteCord {
            store: Store::load(),
            net: None,
            view: View::Friends(Tab::Online),
            name_input,
            link_input,
            setup_error: None,
            composer,
            add_input,
            add_result: None,
            scroller,
            rows: Rc::new(Vec::new()),
            toast: None,
            last_typing: 0,
            _subs: subs,
        };
        if let Some(store) = app.store.as_mut() {
            let last = store.me.last_view.clone();
            if store.me.dms.contains_key(&last) {
                store.unread.remove(&last);
                app.view = View::Dm(last);
            }
            app.connect(cx);
            app.rebuild_rows(cx);
        }
        // A slow tick: clears "is typing…" and old toasts.
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_millis(1000)).await;
            if this.update(cx, |this, cx| this.tick(cx)).is_err() {
                break;
            }
        })
        .detach();
        app
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
                let touched = store.on_text(&t);
                for k in touched {
                    if self.view != View::Dm(k.clone()) {
                        *store.unread.entry(k).or_default() += 1;
                    }
                }
            }
        }
        self.after_change(cx);
    }

    /// Sends what the store queued, shows its notices, and redraws.
    fn after_change(&mut self, cx: &mut Context<Self>) {
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

    fn toast(&mut self, text: &str, ms: i64) {
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
            rows.push(Row {
                msg: m.clone(),
                head,
                day: if new_day { day.map(|d| d.format("%B %-d, %Y").to_string()) } else { None },
            });
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

    fn open_view(&mut self, v: View, window: &mut Window, cx: &mut Context<Self>) {
        if let (View::Dm(k), Some(s)) = (&v, self.store.as_mut()) {
            s.unread.remove(k);
            if let Some(d) = s.me.dms.get_mut(k) {
                d.closed = false;
            }
        }
        let switching = self.view != v;
        self.view = v;
        if let Some(s) = self.store.as_mut() {
            s.me.last_view = match &self.view { View::Dm(k) => k.clone(), _ => String::new() };
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
        self.connect(cx);
        cx.notify();
    }

    fn link(&mut self, cx: &mut Context<Self>) {
        let code = self.link_input.read(cx).value().to_string();
        let name = self.name_input.read(cx).value().trim().to_string();
        match Store::link(&code, if name.is_empty() { "Someone" } else { &name }) {
            Ok(s) => {
                self.store = Some(s);
                self.setup_error = None;
                self.connect(cx);
                self.toast("Linked! Your friends and DMs from the browser are here now.", 5000);
            }
            Err(e) => self.setup_error = Some(e),
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
                    .w(px(480.))
                    .p(px(32.))
                    .rounded(px(8.))
                    .bg(c_main())
                    .shadow_lg()
                    .flex()
                    .flex_col()
                    .gap(px(10.))
                    .child(div().text_2xl().font_weight(FontWeight::BOLD).text_color(c_strong()).text_center().child("Welcome to LiteCord"))
                    .child(div().text_sm().text_color(c_muted()).text_center().mb(px(10.)).child("The native desktop app. No account needed."))
                    .child(label("DISPLAY NAME"))
                    .child(Input::new(&self.name_input))
                    .child(Button::new("fresh").primary().label("Start fresh").on_click(cx.listener(|this, _, _, cx| this.start_fresh(cx))))
                    .child(div().my(px(8.)).h(px(1.)).bg(c_border()))
                    .child(label("OR LINK YOUR BROWSER ACCOUNT"))
                    .child(div().text_sm().text_color(c_muted()).child(
                        "In LiteCord in your browser, open User Settings › Devices › Link the Desktop App, copy the code and paste it here. Your friends and DMs come along.",
                    ))
                    .child(Input::new(&self.link_input))
                    .child(Button::new("link").label("Link").on_click(cx.listener(|this, _, _, cx| this.link(cx))))
                    .children(self.setup_error.clone().map(|e| div().text_sm().text_color(c_red()).child(e))),
            )
            .into_any_element()
    }

    fn render_rail(&self) -> Div {
        let unread: usize = self.store.as_ref().map(|s| s.unread.values().sum()).unwrap_or(0);
        let pending = self.store.as_ref().map(|s| s.me.friends.values().filter(|f| f.state == FriendState::In).count()).unwrap_or(0);
        let badge = unread + pending;
        div()
            .w(px(72.))
            .flex_none()
            .h_full()
            .bg(c_rail())
            .flex()
            .flex_col()
            .items_center()
            .pt(px(12.))
            .gap(px(8.))
            .child(
                div()
                    .relative()
                    .child(
                        div()
                            .size(px(48.))
                            .rounded(px(16.))
                            .bg(c_accent())
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_color(white())
                            .font_weight(FontWeight::BOLD)
                            .child("LC"),
                    )
                    .when(badge > 0, |d| {
                        d.child(
                            div()
                                .absolute()
                                .right(px(-4.))
                                .bottom(px(-4.))
                                .min_w(px(22.))
                                .h(px(22.))
                                .px(px(5.))
                                .rounded_full()
                                .bg(c_red())
                                .border_4()
                                .border_color(c_rail())
                                .text_color(white())
                                .text_xs()
                                .font_weight(FontWeight::BOLD)
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(badge.to_string()),
                        )
                    }),
            )
            .child(div().w(px(32.)).h(px(2.)).rounded_full().bg(c_active()))
            .child(
                div()
                    .id("servers")
                    .size(px(48.))
                    .rounded_full()
                    .bg(c_side())
                    .text_color(c_green())
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(|d| d.rounded(px(16.)).bg(c_green()).text_color(white()))
                    .child(Icon::new(IconName::Globe).size(px(22.)))
                    .tooltip(|window, cx| gpui_kit::component::tooltip::Tooltip::new("Servers open in your browser for now").build(window, cx))
                    .on_click(|_, _, cx| cx.open_url(SERVER)),
            )
    }

    fn render_sidebar(&self, cx: &mut Context<Self>) -> Div {
        let store = self.store.as_ref().unwrap();
        let friends_on = matches!(self.view, View::Friends(_));
        let pending = store.me.friends.values().filter(|f| f.state == FriendState::In).count();
        let mut list = div().id("dmlist").flex_1().min_h(px(0.)).overflow_y_scroll().px(px(8.)).flex().flex_col().gap(px(2.));
        list = list.child(
            div()
                .id("friends")
                .h(px(42.))
                .px(px(12.))
                .rounded(px(6.))
                .flex()
                .items_center()
                .gap(px(12.))
                .cursor_pointer()
                .text_color(if friends_on { c_strong() } else { c_muted() })
                .when(friends_on, |d| d.bg(c_active()))
                .hover(|d| d.bg(c_hover()).text_color(c_text()))
                .child(Icon::new(IconName::User).size(px(20.)))
                .child(div().flex_1().font_weight(FontWeight::MEDIUM).child("Friends"))
                .when(pending > 0, |d| d.child(count_badge(pending)))
                .on_click(cx.listener(|this, _, window, cx| this.open_view(View::Friends(Tab::Online), window, cx))),
        );
        list = list.child(
            div()
                .mt(px(12.))
                .mb(px(4.))
                .px(px(8.))
                .text_xs()
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(c_muted())
                .child("Direct Messages"),
        );
        for key in store.dm_list() {
            let on = self.view == View::Dm(key.clone());
            let title = store.dm_title(&key);
            let unread = store.unread.get(&key).copied().unwrap_or(0);
            let (icon, sub) = match store.dm_other(&key) {
                Some(uid) => (avatar_dot(&uid, &title, 32., store.status_of(&uid), if on { c_active() } else { c_side() }), store.subline(&uid)),
                None => {
                    let n = store.me.dms.get(&key).map(|d| d.members.len()).unwrap_or(0);
                    (avatar(&key, &title, 32.), format!("{n} Members"))
                }
            };
            let k = key.clone();
            list = list.child(
                div()
                    .id(SharedString::from(format!("dm-{key}")))
                    .h(px(42.))
                    .px(px(8.))
                    .rounded(px(6.))
                    .flex()
                    .items_center()
                    .gap(px(12.))
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
                    .on_click(cx.listener(move |this, _, window, cx| this.open_view(View::Dm(k.clone()), window, cx))),
            );
        }
        if store.dm_list().is_empty() {
            list = list.child(div().px(px(8.)).text_sm().text_color(c_muted()).child("Your DMs show up here. Message a friend to start one."));
        }
        let (dot, line) = if store.online { ("online", "Online") } else { ("offline", "Connecting…") };
        div()
            .w(px(240.))
            .flex_none()
            .h_full()
            .bg(c_side())
            .flex()
            .flex_col()
            .child(
                div().h(px(48.)).flex_none().px(px(10.)).flex().items_center().border_b_1().border_color(c_border()).child(
                    div()
                        .id("find")
                        .w_full()
                        .h(px(28.))
                        .px(px(8.))
                        .rounded(px(6.))
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
            )
            .child(div().h(px(8.)))
            .child(list)
            // The user bar, floating like Discord's.
            .child(
                div().p(px(8.)).flex_none().child(
                    div()
                        .h(px(52.))
                        .px(px(8.))
                        .rounded(px(8.))
                        .bg(c_panel())
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(avatar_dot(&store.me.uid, &store.me.name, 32., dot, c_panel()))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(0.))
                                .flex()
                                .flex_col()
                                .child(div().truncate().text_sm().font_weight(FontWeight::SEMIBOLD).text_color(c_strong()).child(store.me.name.clone()))
                                .child(div().truncate().text_xs().text_color(c_muted()).child(line)),
                        )
                        .child(
                            Button::new("copyid")
                                .ghost()
                                .compact()
                                .icon(IconName::Copy)
                                .tooltip("Copy your friend code")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if let Some(s) = &this.store {
                                        cx.write_to_clipboard(ClipboardItem::new_string(s.me.uid.clone()));
                                        this.toast("Your friend code was copied. Friends can paste it in Add Friend.", 4000);
                                        cx.notify();
                                    }
                                })),
                        ),
                ),
            )
    }

    fn header(&self, title: impl IntoElement) -> Div {
        div()
            .h(px(48.))
            .flex_none()
            .px(px(16.))
            .flex()
            .items_center()
            .gap(px(12.))
            .border_b_1()
            .border_color(c_border())
            .child(title)
    }

    fn render_friends(&mut self, tab: Tab, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let _ = window;
        let store = self.store.as_ref().unwrap();
        let pending = store.me.friends.values().filter(|f| f.state == FriendState::In).count();
        let tab_btn = |id: &'static str, label: &'static str, t: Tab, cx: &mut Context<Self>| {
            let on = tab == t;
            div()
                .id(id)
                .px(px(10.))
                .py(px(2.))
                .rounded(px(6.))
                .cursor_pointer()
                .font_weight(FontWeight::MEDIUM)
                .text_color(if on { c_strong() } else { c_muted() })
                .when(on, |d| d.bg(c_active()))
                .hover(|d| d.bg(c_hover()).text_color(c_text()))
                .flex()
                .items_center()
                .gap(px(6.))
                .child(label)
                .when(t == Tab::Pending && pending > 0, |d| d.child(count_badge(pending)))
                .on_click(cx.listener(move |this, _, window, cx| this.open_view(View::Friends(t), window, cx)))
        };
        let head = self.header(
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(Icon::new(IconName::User).size(px(20.)).text_color(c_muted()))
                .child(div().font_weight(FontWeight::SEMIBOLD).text_color(c_strong()).child("Friends"))
                .child(div().mx(px(8.)).w(px(4.)).h(px(4.)).rounded_full().bg(c_muted()))
                .child(tab_btn("t-on", "Online", Tab::Online, cx))
                .child(tab_btn("t-all", "All", Tab::All, cx))
                .child(tab_btn("t-pend", "Pending", Tab::Pending, cx))
                .child(tab_btn("t-blk", "Blocked", Tab::Blocked, cx))
                .child(
                    div()
                        .id("t-add")
                        .px(px(10.))
                        .py(px(2.))
                        .rounded(px(6.))
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
                .p(px(20.))
                .flex()
                .flex_col()
                .gap(px(8.))
                .child(div().text_lg().font_weight(FontWeight::BOLD).text_color(c_strong()).child("Add Friend"))
                .child(div().text_sm().text_color(c_muted()).child("You can add friends with their LiteCord username or friend code."))
                .child(
                    div()
                        .flex()
                        .gap(px(8.))
                        .items_center()
                        .child(div().flex_1().child(Input::new(&self.add_input)))
                        .child(Button::new("sendreq").primary().label("Send Friend Request").on_click(cx.listener(|this, _, window, cx| this.send_friend_request(window, cx)))),
                )
                .children(result.map(|(ok, t)| div().text_sm().text_color(if ok { c_green() } else { c_red() }).child(t)))
                .child(div().mt(px(16.)).text_sm().text_color(c_muted()).child(format!("Your friend code: {}", store.me.uid)))
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
                .map(|(u, f)| (u.clone(), { let n = store.name_of(u); if n == "Someone" { f.name.clone() } else { n } }, f.state))
                .collect();
            people.sort_by_key(|(_, n, _)| n.to_lowercase());
            let title = match tab {
                Tab::Online => "Online",
                Tab::All => "All Friends",
                Tab::Pending => "Pending",
                _ => "Blocked",
            };
            let mut list = div().id("friendlist").flex_1().overflow_y_scroll().px(px(20.)).flex().flex_col();
            list = list.child(
                div().pt(px(16.)).pb(px(8.)).text_xs().font_weight(FontWeight::SEMIBOLD).text_color(c_muted()).child(format!("{title} — {}", people.len())),
            );
            if people.is_empty() {
                let empty = match tab {
                    Tab::Online => "No one's around to play with Wumpus.",
                    Tab::Pending => "There are no pending friend requests.",
                    Tab::Blocked => "You haven't blocked anyone.",
                    _ => "You don't have any friends yet. Add some with Add Friend!",
                };
                list = list.child(div().pt(px(60.)).text_center().text_color(c_muted()).child(empty.replace("Wumpus", "LiteCord")));
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
                let mut actions = div().flex().gap(px(8.));
                let u = uid.clone();
                match state {
                    FriendState::Friend => {
                        let u2 = uid.clone();
                        actions = actions
                            .child(round_btn(SharedString::from(format!("msg-{uid}")), IconName::Inbox, "Message").on_click(cx.listener(move |this, _, window, cx| {
                                let key = this.store.as_mut().map(|s| s.dm_for(&u));
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
                                    s.accept_friend(&u);
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
                        actions = actions.child(round_btn(SharedString::from(format!("rm-{uid}")), IconName::Close, if state == FriendState::Out { "Cancel" } else { "Unblock" }).on_click(
                            cx.listener(move |this, _, _, cx| {
                                if let Some(s) = this.store.as_mut() {
                                    s.remove_friend(&u);
                                }
                                this.after_change(cx);
                            }),
                        ));
                    }
                }
                list = list.child(
                    div()
                        .id(SharedString::from(format!("f-{uid}")))
                        .h(px(62.))
                        .px(px(10.))
                        .border_t_1()
                        .border_color(c_border())
                        .rounded(px(8.))
                        .flex()
                        .items_center()
                        .gap(px(12.))
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
            list.into_any_element().into_element_div()
        };
        div().flex_1().min_w(px(0.)).h_full().bg(c_main()).flex().flex_col().child(head).child(body)
    }

    fn render_dm(&mut self, key: String, cx: &mut Context<Self>) -> Div {
        let store = self.store.as_ref().unwrap();
        let title = store.dm_title(&key);
        let other = store.dm_other(&key);
        let typing = store.typing_in(&key);
        let rows = self.rows.clone();
        let my_uid = store.me.uid.clone();
        let names: Rc<std::collections::HashMap<String, String>> = Rc::new(
            rows.iter().map(|r| r.msg.uid.clone()).filter(|u| !u.is_empty()).map(|u| { let n = store.name_of(&u); (u, n) }).collect(),
        );
        let intro_title = title.clone();
        let intro_uid = other.clone().unwrap_or_else(|| key.clone());
        let is_group = other.is_none();
        let head = self.header(
            div()
                .flex()
                .items_center()
                .gap(px(10.))
                .child(match &other {
                    Some(u) => avatar_dot(u, &title, 24., store.status_of(u), c_main()),
                    None => avatar(&key, &title, 24.),
                })
                .child(div().font_weight(FontWeight::SEMIBOLD).text_color(c_strong()).child(title.clone()))
                .children(other.as_ref().map(|u| div().text_sm().text_color(c_muted()).child(status_label(store.status_of(u))))),
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
                        .mx(px(16.))
                        .my(px(8.))
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(div().flex_1().h(px(1.)).bg(c_border()))
                        .child(div().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(c_muted()).child(day.clone()))
                        .child(div().flex_1().h(px(1.)).bg(c_border())),
                );
            }
            if m.sys {
                return out.child(
                    div()
                        .px(px(16.))
                        .py(px(4.))
                        .flex()
                        .gap(px(8.))
                        .text_sm()
                        .text_color(c_muted())
                        .child(div().text_color(c_green()).child("→"))
                        .child(m.text.clone())
                        .child(div().text_xs().child(when(m.ts))),
                );
            }
            let name = if m.uid == my_uid { m.name.clone() } else { names.get(&m.uid).cloned().unwrap_or_else(|| m.name.clone()) };
            let mut body = div().text_color(c_text()).child(m.text.clone());
            if let Some(f) = &m.file {
                body = body.child(div().text_sm().text_color(c_muted()).child(format!("📎 {f} (open it in the browser for now)")));
            }
            if m.edited.is_some() {
                body = body.child(div().text_xs().text_color(c_muted()).child("(edited)"));
            }
            let line = div().px(px(16.)).pr(px(48.)).hover(|d| d.bg(c_msghover())).flex().gap(px(16.));
            out.child(if r.head {
                line.mt(px(16.))
                    .py(px(2.))
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
                                    .gap(px(8.))
                                    .child(div().font_weight(FontWeight::MEDIUM).text_color(hsla(hue(&m.uid) / 360., 0.6, 0.75, 1.)).child(name))
                                    .child(div().text_xs().text_color(c_muted()).child(when(m.ts))),
                            )
                            .child(body),
                    )
            } else {
                line.py(px(1.)).child(div().w(px(40.)).flex_none()).child(div().flex_1().min_w(px(0.)).child(body))
            })
        })
        .scrollbar(true)
        .with_bottom_fade(c_main())
        .with_row_style(StyleRefinement::default().pb_0())
        .with_list_style(StyleRefinement::default().pb(px(12.)))
        .flex_1()
        .min_h(px(0.));
        let typing_line = match typing.len() {
            0 => String::new(),
            1 => format!("{} is typing…", typing[0]),
            2 => format!("{} and {} are typing…", typing[0], typing[1]),
            _ => "Several people are typing…".into(),
        };
        let _ = cx;
        div()
            .flex_1()
            .min_w(px(0.))
            .h_full()
            .bg(c_main())
            .flex()
            .flex_col()
            .child(head)
            .child(if row_count == 0 {
                div().flex_1().flex().flex_col().justify_end().child(dm_intro(other.as_deref().unwrap_or(&key), &title, other.is_none())).into_any_element()
            } else {
                scroller.into_any_element()
            })
            .child(
                div().px(px(16.)).flex_none().child(
                    div().px(px(12.)).py(px(4.)).rounded(px(8.)).bg(c_input()).child(Input::new(&self.composer).appearance(false)),
                ),
            )
            .child(div().h(px(24.)).px(px(16.)).flex_none().flex().items_center().text_xs().text_color(c_text()).child(typing_line))
    }
}

/// Discord's "This is the beginning of your direct message history with …" header.
fn dm_intro(uid: &str, title: &str, group: bool) -> Div {
    div()
        .px(px(16.))
        .pt(px(16.))
        .pb(px(8.))
        .flex()
        .flex_col()
        .gap(px(8.))
        .child(avatar(uid, title, 80.))
        .child(div().text_3xl().font_weight(FontWeight::BOLD).text_color(c_strong()).child(title.to_string()))
        .child(div().text_color(c_muted()).child(if group {
            format!("Welcome to the beginning of the {title} group.")
        } else {
            format!("This is the beginning of your direct message history with {title}.")
        }))
}

fn count_badge(n: usize) -> Div {
    div()
        .min_w(px(16.))
        .h(px(16.))
        .px(px(4.))
        .rounded_full()
        .bg(c_red())
        .text_color(white())
        .text_size(px(11.))
        .font_weight(FontWeight::BOLD)
        .flex()
        .items_center()
        .justify_center()
        .child(n.to_string())
}

fn round_btn(id: SharedString, icon: IconName, tip: &'static str) -> Button {
    Button::new(id).ghost().icon(icon).tooltip(tip).rounded(ButtonRounded::Large)
}

trait IntoDiv {
    fn into_element_div(self) -> Div;
}
impl IntoDiv for AnyElement {
    fn into_element_div(self) -> Div {
        div().flex_1().min_h(px(0.)).flex().flex_col().child(self)
    }
}

impl Render for LiteCord {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if self.store.is_none() {
            return div().size_full().text_color(c_text()).child(self.render_setup(cx)).into_any_element();
        }
        let main = match self.view.clone() {
            View::Friends(t) => self.render_friends(t, window, cx),
            View::Dm(k) => self.render_dm(k, cx),
        };
        let toast = self.toast.clone();
        div()
            .size_full()
            .relative()
            .flex()
            .bg(c_rail())
            .text_color(c_text())
            .text_size(px(15.))
            .child(self.render_rail())
            .child(self.render_sidebar(cx))
            .child(main)
            .children(toast.map(|(t, _)| {
                div().absolute().bottom(px(80.)).left_0().right_0().flex().justify_center().child(
                    div().max_w(px(520.)).px(px(16.)).py(px(10.)).rounded(px(8.)).bg(rgb(0x111113)).shadow_lg().text_color(c_strong()).text_sm().child(t),
                )
            }))
            .into_any_element()
    }
}

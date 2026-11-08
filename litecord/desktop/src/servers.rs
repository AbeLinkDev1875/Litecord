//! Servers: the same rooms as the web app's (`?room=<name>`). Joining one connects to its relay
//! room through the call engine, which speaks the web app's room protocol: channels and chat
//! history come from whoever hosts the room (or from us, if we're the first one there), and
//! voice channels work like DM calls. Our copy of each server is saved in `rooms/`.

use chrono::{Local, TimeZone};
use gpui_kit::component::input::{Input, InputState};
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use gpui_kit::white;

use crate::app::{avatar, LiteCord, View};
use crate::call::{ChatView, Cmd};
use crate::callui::call_icon;
use crate::prefs::*;

#[derive(Default)]
pub struct ServerUi {
    /// The text or voice channel shown, by server.
    pub ch: std::collections::HashMap<String, String>,
    /// The "Join a Server" box is open.
    pub adding: bool,
    pub error: Option<String>,
}

/// Room names work like the web app's join box: trimmed, at most 32 characters.
pub fn clean_room(s: &str) -> Option<String> {
    let r: String = s.trim().chars().take(32).collect();
    if r.is_empty() || r.starts_with("dm-") { None } else { Some(r) }
}

/// Two letters for a server's icon, like Discord's.
fn initials(name: &str) -> String {
    let words: Vec<&str> = name.split(|c: char| !c.is_alphanumeric()).filter(|w| !w.is_empty()).collect();
    let s: String = match words.len() {
        0 => name.chars().take(2).collect(),
        1 => words[0].chars().take(2).collect(),
        _ => words.iter().take(2).filter_map(|w| w.chars().next()).collect(),
    };
    s.to_uppercase()
}

impl LiteCord {
    /// The room the engine has open, if it's a server (not a DM call room).
    pub(crate) fn server_room(&self) -> Option<String> {
        self.call.view.room.clone().filter(|r| !r.starts_with("dm-"))
    }

    /// Opens a server: connects to its room unless we're in a call somewhere else.
    pub(crate) fn open_server(&mut self, room: &str, window: &mut Window, cx: &mut Context<Self>) {
        let here = self.call.view.room.as_deref() == Some(room);
        if !here && self.call.view.voice.is_some() {
            self.toast("You're in a call. Leave it first to open another server.", 5000);
            cx.notify();
            return;
        }
        if !self.prefs.servers.iter().any(|r| r == room) {
            self.prefs.servers.push(room.to_string());
            self.save_prefs();
        }
        if !here {
            let Some(store) = &self.store else { return };
            let (uid, name, status, custom) = (store.me.uid.clone(), store.me.name.clone(), self.prefs.status.clone(), self.prefs.custom.clone());
            self.call.dm = None;
            let saved = crate::rooms::load(room);
            self.call_send(Cmd::Open { room: room.into(), uid, name, status, custom, saved }, cx);
        }
        self.open_view(View::Server(room.to_string()), window, cx);
    }

    /// Leaves a server for good (removes it from the rail).
    pub(crate) fn forget_server(&mut self, room: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.call.view.room.as_deref() == Some(room) {
            if self.call.view.voice.is_some() {
                self.leave_call(cx);
            }
            self.call_send(Cmd::Close, cx);
        }
        self.prefs.servers.retain(|r| r != room);
        self.save_prefs();
        self.server.ch.remove(room);
        self.open_view(View::Friends(crate::app::Tab::Online), window, cx);
    }

    pub(crate) fn join_server_voice(&mut self, ch: &str, cx: &mut Context<Self>) {
        let Some(room) = self.server_room() else { return };
        if self.call.view.voice.as_deref() == Some(ch) {
            return;
        }
        let (m, d) = (self.call.muted, self.call.deaf);
        self.call_send(Cmd::SetMuted(m), cx);
        self.call_send(Cmd::SetDeaf(d), cx);
        self.call_send(Cmd::JoinVoice(ch.into()), cx);
        self.server.ch.insert(room, ch.to_string());
        cx.notify();
    }

    pub(crate) fn open_server_call(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let (Some(room), Some(v)) = (self.server_room(), self.call.view.voice.clone()) else { return };
        self.server.ch.insert(room.clone(), v);
        self.open_view(View::Server(room), window, cx);
    }

    fn server_channel(&self, room: &str) -> Option<crate::call::ChannelView> {
        let chans = &self.call.view.channels;
        let want = self.server.ch.get(room);
        want.and_then(|w| chans.iter().find(|c| &c.id == w)).or_else(|| chans.iter().find(|c| c.kind == "text")).cloned()
    }

    /// Server icons in the rail, and the "+" to join one.
    pub(crate) fn rail_servers(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mut out = Vec::new();
        for room in &self.prefs.servers {
            let on = self.view == View::Server(room.clone());
            let r = room.clone();
            let tip: SharedString = room.clone().into();
            let voice_here = self.call.view.room.as_deref() == Some(room.as_str()) && self.call.view.voice.is_some();
            out.push(
                div().relative().w_full().flex().justify_center()
                    .when(on, |d| d.child(div().absolute().left_0().top(u(4.)).w(u(4.)).h(u(40.)).rounded_r(u(4.)).bg(c_strong())))
                    .child(
                        div().id(SharedString::from(format!("srv-{room}"))).relative().size(u(48.)).rounded(if on { u(16.) } else { u(24.) })
                            .bg(if on { c_accent() } else { c_side() }).text_color(if on { white() } else { c_text() })
                            .flex().items_center().justify_center().font_weight(FontWeight::SEMIBOLD).text_size(u(16.)).cursor_pointer()
                            .hover(|d| d.rounded(u(16.)).bg(c_accent()).text_color(white()))
                            .child(initials(room))
                            .when(voice_here, |d| d.child(div().absolute().right(u(-2.)).bottom(u(-2.)).size(u(18.)).rounded_full().bg(c_green()).border(u(3.)).border_color(c_rail())))
                            .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
                            .on_click(cx.listener(move |this, _, window, cx| this.open_server(&r, window, cx))),
                    )
                    .into_any_element(),
            );
        }
        out.push(
            div().id("addserver").size(u(48.)).rounded(u(24.)).bg(c_side()).text_color(c_green()).flex().items_center().justify_center().cursor_pointer()
                .hover(|d| d.rounded(u(16.)).bg(c_green()).text_color(white()))
                .child(Icon::new(IconName::Plus).size(u(22.)))
                .tooltip(|window, cx| Tooltip::new("Join or Create a Server").build(window, cx))
                .on_click(cx.listener(|this, _, window, cx| {
                    this.server.adding = true;
                    this.server.error = None;
                    this.server_name_input.update(cx, |s, cx| {
                        s.set_value("", window, cx);
                        s.focus(window, cx);
                    });
                    cx.notify();
                }))
                .into_any_element(),
        );
        out
    }

    /// The "Join a Server" box.
    pub(crate) fn render_add_server(&self, cx: &mut Context<Self>) -> Div {
        div().absolute().size_full().flex().items_center().justify_center().bg(hsla(0., 0., 0., 0.6)).child(
            div().id("addbox").w(u(440.)).rounded(u(10.)).bg(c_main()).border_1().border_color(c_border()).shadow_lg().p(u(20.)).flex().flex_col().gap(u(12.))
                .on_click(|_, _, cx| cx.stop_propagation())
                .child(div().text_size(u(22.)).font_weight(FontWeight::BOLD).text_color(c_strong()).child("Join a Server"))
                .child(div().text_sm().text_color(c_muted()).child("Type the server's name, the same one people use in the web app. If nobody has made it yet, you'll create it."))
                .child(div().text_xs().font_weight(FontWeight::BOLD).text_color(c_muted()).child("SERVER NAME"))
                .child(div().px(u(10.)).py(u(4.)).rounded(u(6.)).bg(c_input()).child(Input::new(&self.server_name_input).appearance(false)))
                .children(self.server.error.clone().map(|e| div().text_sm().text_color(c_red()).child(e)))
                .child(
                    div().flex().justify_end().gap(u(8.))
                        .child(div().id("addcancel").px(u(16.)).py(u(8.)).rounded(u(6.)).cursor_pointer().text_color(c_text()).hover(|d| d.bg(c_hover())).child("Cancel")
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.server.adding = false;
                                cx.notify();
                            })))
                        .child(div().id("addjoin").px(u(16.)).py(u(8.)).rounded(u(6.)).cursor_pointer().bg(c_accent()).text_color(white()).font_weight(FontWeight::SEMIBOLD).child("Join Server")
                            .on_click(cx.listener(|this, _, window, cx| this.submit_add_server(window, cx)))),
                ),
        )
    }

    pub(crate) fn submit_add_server(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.server_name_input.read(cx).value().to_string();
        match clean_room(&text) {
            Some(room) => {
                self.server.adding = false;
                self.open_server(&room, window, cx);
            }
            None => {
                self.server.error = Some("Type a server name.".into());
                cx.notify();
            }
        }
    }

    /// The channel list for the server sidebar.
    pub(crate) fn server_sidebar(&self, room: &str, cx: &mut Context<Self>) -> (Div, Stateful<Div>) {
        let v = &self.call.view;
        let connected = v.room.as_deref() == Some(room);
        let r2 = room.to_string();
        let head = div().h(u(48.)).flex_none().px(u(16.)).flex().items_center().gap(u(8.)).border_b_1().border_color(c_border())
            .child(div().flex_1().min_w(px(0.)).truncate().font_weight(FontWeight::BOLD).text_color(c_strong()).child(room.to_string()))
            .child(
                div().id("leavesrv").size(u(28.)).rounded(u(6.)).flex().items_center().justify_center().cursor_pointer().text_color(c_muted())
                    .hover(|d| d.bg(c_hover()).text_color(c_red()))
                    .tooltip(|window, cx| Tooltip::new("Leave Server").build(window, cx))
                    .child(Icon::new(IconName::Close).size(u(16.)))
                    .on_click(cx.listener(move |this, _, window, cx| this.forget_server(&r2, window, cx))),
            );
        let mut list = div().id("chlist").flex_1().min_h(px(0.)).overflow_y_scroll().px(u(8.)).pt(u(12.)).flex().flex_col().gap(u(2.));
        if !connected || !v.ready {
            list = list.child(div().px(u(8.)).text_sm().text_color(c_muted()).child(if v.connected || !connected { "Connecting…" } else { "Reconnecting…" }));
            return (head, list);
        }
        let shown = self.server_channel(room).map(|c| c.id);
        let section = |t: &'static str| div().mt(u(12.)).mb(u(2.)).px(u(8.)).text_xs().font_weight(FontWeight::SEMIBOLD).text_color(c_muted()).child(t);
        let text: Vec<_> = v.channels.iter().filter(|c| c.kind == "text" || c.kind == "forum").cloned().collect();
        let voice: Vec<_> = v.channels.iter().filter(|c| c.kind == "voice" || c.kind == "stage").cloned().collect();
        if !text.is_empty() {
            list = list.child(section("TEXT CHANNELS"));
        }
        for c in text {
            let on = shown.as_deref() == Some(c.id.as_str());
            let (id, room2, forum) = (c.id.clone(), room.to_string(), c.kind == "forum");
            list = list.child(
                div().id(SharedString::from(format!("ch-{}", c.id))).h(u(34.)).px(u(8.)).rounded(u(6.)).flex().items_center().gap(u(6.)).cursor_pointer()
                    .text_color(if on { c_strong() } else { c_muted() }).when(on, |d| d.bg(c_active())).hover(|d| d.bg(c_hover()).text_color(c_text()))
                    .child(div().w(u(20.)).flex().justify_center().text_size(u(18.)).child(if forum { "≡" } else { "#" }))
                    .child(div().flex_1().truncate().font_weight(FontWeight::MEDIUM).child(c.name.clone()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if forum {
                            this.toast("Forums open in the browser for now.", 4000);
                            cx.open_url(&format!("{}/?room={}", crate::app::SERVER, room2));
                        } else {
                            this.server.ch.insert(room2.clone(), id.clone());
                        }
                        cx.notify();
                    })),
            );
        }
        if !voice.is_empty() {
            list = list.child(section("VOICE CHANNELS"));
        }
        for c in voice {
            let on = shown.as_deref() == Some(c.id.as_str());
            let (id, room2) = (c.id.clone(), room.to_string());
            list = list.child(
                div().id(SharedString::from(format!("ch-{}", c.id))).h(u(34.)).px(u(8.)).rounded(u(6.)).flex().items_center().gap(u(6.)).cursor_pointer()
                    .text_color(if on { c_strong() } else { c_muted() }).when(on, |d| d.bg(c_active())).hover(|d| d.bg(c_hover()).text_color(c_text()))
                    .child(div().w(u(20.)).flex().justify_center().child(call_icon(if c.kind == "stage" { "signal" } else { "head" }).size(u(18.))))
                    .child(div().flex_1().truncate().font_weight(FontWeight::MEDIUM).child(c.name.clone()))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.server.ch.insert(room2.clone(), id.clone());
                        this.join_server_voice(&id, cx);
                    })),
            );
            for p in v.in_voice(&c.id) {
                let speaking = v.speaking.contains(&p.id) && !p.state.muted;
                list = list.child(
                    div().h(u(30.)).pl(u(34.)).pr(u(8.)).flex().items_center().gap(u(8.)).text_sm().text_color(c_muted())
                        .child(div().rounded_full().border(u(2.)).border_color(if speaking { c_green() } else { transparent_black() }).child(avatar(if p.uid.is_empty() { &p.id } else { &p.uid }, &p.name, 22.)))
                        .child(div().flex_1().truncate().child(p.name.clone()))
                        .when(p.state.screen.is_some(), |d| d.child(div().px(u(4.)).rounded(u(4.)).bg(c_red()).text_color(white()).text_size(u(10.)).font_weight(FontWeight::BOLD).child("LIVE")))
                        .when(p.state.deaf || p.state.muted, |d| d.child(call_icon(if p.state.deaf { "headOff" } else { "micOff" }).size(u(16.)).text_color(c_red()))),
                );
            }
        }
        (head, list)
    }

    /// The main area of a server: a text channel's chat, or a voice channel's call.
    pub(crate) fn render_server(&mut self, room: String, cx: &mut Context<Self>) -> Div {
        let base = div().flex_1().min_w(px(0.)).h_full().bg(c_main()).flex().flex_col();
        let v = &self.call.view;
        if v.room.as_deref() != Some(room.as_str()) || !v.ready {
            return base.child(self.header(div().font_weight(FontWeight::SEMIBOLD).text_color(c_strong()).child(room.clone())))
                .child(div().flex_1().flex().items_center().justify_center().text_color(c_muted()).child("Connecting to the server…"));
        }
        let Some(ch) = self.server_channel(&room) else {
            return base.child(div().flex_1().flex().items_center().justify_center().text_color(c_muted()).child("This server has no channels."));
        };
        if ch.kind == "voice" || ch.kind == "stage" {
            let head = self.header(div().flex().items_center().gap(u(8.)).child(call_icon("head").size(u(20.)).text_color(c_muted())).child(div().font_weight(FontWeight::SEMIBOLD).text_color(c_strong()).child(ch.name.clone())));
            return base.child(head).child(self.render_call_area(&ch.id, None, None, cx));
        }
        // A text channel.
        let n = v.history.get(&ch.id).map(|h| h.len()).unwrap_or(0);
        if self.server_rows_for.as_deref() != Some(ch.id.as_str()) || self.server_rows_len != n {
            self.rebuild_server_rows(&ch.id, cx);
        }
        let v = &self.call.view;
        let rows = self.server_rows.clone();
        let h24 = self.prefs.clock24();
        let font = self.prefs.chat_font as f32;
        let scroller = gpui_kit::component::message_scroller::MessageScroller::new("srvmsgs", self.server_scroller.clone(), move |ix, _, _| {
            let (m, head): &(ChatView, bool) = &rows[ix];
            let when = Local.timestamp_millis_opt(m.ts).single().map(|t| if h24 { t.format("%d/%m/%Y %H:%M").to_string() } else { t.format("%m/%d/%Y %-I:%M %p").to_string() }).unwrap_or_default();
            let body = div().text_size(u(font)).text_color(c_text()).child(if m.text.is_empty() { m.file.clone().map(|f| format!("📎 {f}")).unwrap_or_default() } else { m.text.clone() })
                .when(m.edited, |d| d.child(div().text_xs().text_color(c_muted()).child("(edited)")));
            let line = div().px(u(16.)).flex().gap(u(16.));
            if *head {
                line.mt(u(12.)).child(avatar(if m.uid.is_empty() { &m.name } else { &m.uid }, &m.name, 40.))
                    .child(div().flex_1().min_w(px(0.)).flex().flex_col()
                        .child(div().flex().items_center().gap(u(8.)).child(div().font_weight(FontWeight::MEDIUM).text_color(c_strong()).child(m.name.clone())).child(div().text_xs().text_color(c_muted()).child(when)))
                        .child(body))
            } else {
                line.child(div().w(u(40.)).flex_none()).child(div().flex_1().min_w(px(0.)).child(body))
            }
        })
        .scrollbar(true)
        .with_bottom_fade(c_main())
        .flex_1()
        .min_h(px(0.));
        let typing: Vec<String> = v.typing.iter().filter(|(c, _)| *c == ch.id).map(|(_, n)| n.clone()).collect();
        let typing_line = match typing.len() {
            0 => String::new(),
            1 => format!("{} is typing…", typing[0]),
            _ => "Several people are typing…".into(),
        };
        let head = self.header(div().flex().items_center().gap(u(8.)).child(div().text_size(u(22.)).text_color(c_muted()).child("#")).child(div().font_weight(FontWeight::SEMIBOLD).text_color(c_strong()).child(ch.name.clone())));
        base.child(head)
            .child(if rows_empty(&self.server_rows) {
                div().flex_1().flex().flex_col().justify_end().px(u(16.)).pb(u(12.)).gap(u(6.))
                    .child(div().text_3xl().font_weight(FontWeight::BOLD).text_color(c_strong()).child(format!("Welcome to #{}!", ch.name)))
                    .child(div().text_color(c_muted()).child(format!("This is the start of the #{} channel.", ch.name)))
                    .into_any_element()
            } else {
                scroller.into_any_element()
            })
            .child(div().px(u(16.)).flex_none().child(div().px(u(12.)).py(u(4.)).rounded(u(8.)).bg(c_input()).text_size(u(font)).child(Input::new(&self.server_composer).appearance(false))))
            .child(div().h(u(24.)).px(u(16.)).flex_none().flex().items_center().text_xs().text_color(c_text()).child(typing_line))
    }

    fn rebuild_server_rows(&mut self, ch: &str, cx: &mut Context<Self>) {
        let list = self.call.view.history.get(ch).cloned().unwrap_or_default();
        let mut rows: Vec<(ChatView, bool)> = Vec::with_capacity(list.len());
        let mut prev: Option<&ChatView> = None;
        for m in &list {
            let head = prev.map_or(true, |p| p.uid != m.uid || p.name != m.name || m.ts - p.ts > 7 * 60_000);
            rows.push((m.clone(), head));
            prev = Some(m);
        }
        let same = self.server_rows_for.as_deref() == Some(ch);
        let (old, new) = (self.server_rows.len(), rows.len());
        self.server_rows = std::rc::Rc::new(rows);
        self.server_rows_for = Some(ch.to_string());
        self.server_rows_len = list.len();
        self.server_scroller.update(cx, |s, cx| {
            if same && s.item_count() == old && new > old {
                s.append(new - old, cx);
            } else {
                s.reset(new, cx);
            }
        });
    }

    /// Enter in a server's message box.
    pub(crate) fn send_server_message(&mut self, input: &Entity<InputState>, window: &mut Window, cx: &mut Context<Self>) {
        let View::Server(room) = self.view.clone() else { return };
        let Some(ch) = self.server_channel(&room) else { return };
        let text = input.read(cx).value().to_string();
        if text.trim().is_empty() {
            return;
        }
        self.call_send(Cmd::Chat { ch: ch.id, text }, cx);
        input.update(cx, |s, cx| s.set_value("", window, cx));
    }

    pub(crate) fn server_typing(&mut self, cx: &mut Context<Self>) {
        let View::Server(room) = self.view.clone() else { return };
        let Some(ch) = self.server_channel(&room) else { return };
        let now = crate::store::now_ms();
        if now - self.last_typing > 4000 {
            self.last_typing = now;
            self.call_send(Cmd::Typing(ch.id), cx);
        }
    }
}

fn rows_empty(r: &std::rc::Rc<Vec<(ChatView, bool)>>) -> bool {
    r.is_empty()
}

#[cfg(test)]
mod tests {
    use super::{clean_room, initials};

    #[test]
    fn room_names() {
        assert_eq!(clean_room("  my server "), Some("my server".into()));
        assert_eq!(clean_room("dm-abc"), None);
        assert_eq!(clean_room("   "), None);
        assert_eq!(initials("lobby"), "LO");
        assert_eq!(initials("Cool Gamers"), "CG");
    }
}

//! The call UI, drawn like the web app's: the call area with everyone's tiles and the call bar
//! (mic, camera, screen share, disconnect), the voice panel above your user bar, the mic and
//! headphone buttons, and the incoming call card. The calls themselves run in `call/`.

use std::collections::HashMap;
use std::sync::Arc;

use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::component::*;
use gpui_kit::*;
use gpui_kit::{prelude::FluentBuilder as _, white};

use crate::app::{avatar, hue, LiteCord, SERVER};
use crate::call::{self, CallEvent, CallView, Cmd, PersonView, DM_VOICE};
use crate::prefs::*;
use crate::store::now_ms;

/// The web app's icons (24×24, drawn with 2px lines).
pub fn call_icon(name: &str) -> Icon {
    const MIC: &str = "M9 5a3 3 0 0 1 6 0v6a3 3 0 0 1-6 0z M5 11a7 7 0 0 0 14 0 M12 18v3";
    const HEAD: &str = "M4 16v-4a8 8 0 0 1 16 0v4 M4 15h3v6H5a1 1 0 0 1-1-1z M20 15h-3v6h2a1 1 0 0 0 1-1z";
    const CAMERA: &str = "M3 7h12v10H3z M15 10l6-3v10l-6-3z";
    let d = match name {
        "mic" => MIC.to_string(),
        "micOff" => format!("{MIC} M3 3l18 18"),
        "head" => HEAD.to_string(),
        "headOff" => format!("{HEAD} M3 3l18 18"),
        "camera" => CAMERA.to_string(),
        "cameraOff" => format!("{CAMERA} M2 3l19 18"),
        "hangup" => "M3 14c5-4 13-4 18 0l-1.5 3-4-1.2V13c-2-.7-5-.7-7 0v2.8L4.5 17z".into(),
        "screen" => "M3 4h18v12H3z M8 20h8 M12 16v4".into(),
        "share" => "M3 4h18v12H3z M8 20h8 M12 16v4 M9 10l3-3 3 3 M12 7v6".into(),
        "signal" => "M5 20v-3 M10 20v-7 M15 20V9 M20 20V5".into(),
        "phone" => "M5 4h4l2 5-2.5 1.5a11 11 0 0 0 5 5L15 13l5 2v4a2 2 0 0 1-2 2A16 16 0 0 1 3 6a2 2 0 0 1 2-2z".into(),
        _ => "M6 6l12 12 M18 6L6 18".into(),
    };
    let svg = format!(r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path d="{d}" fill="none" stroke="#000" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/></svg>"##);
    Icon::default().data(svg.as_bytes())
}

/// The call state the app keeps (the engine is started the first time you call).
#[derive(Default)]
pub struct CallUi {
    pub engine: Option<call::CallHandle>,
    pub view: CallView,
    /// The DM whose call room is open.
    pub dm: Option<String>,
    /// We're ringing the others until then.
    pub calling: Option<i64>,
    /// Ring them once we're in (video or not).
    pub ring_when_in: Option<bool>,
    /// Turn the camera on once we're in.
    pub cam_when_in: bool,
    pub muted: bool,
    pub deaf: bool,
    /// Pictures for the video tiles, made from the engine's frames: key -> (seq, image).
    pub images: HashMap<String, (u64, Arc<RenderImage>)>,
    /// The tile shown big (click a tile to focus it).
    pub focus: Option<String>,
}

impl LiteCord {
    fn engine(&mut self, cx: &mut Context<Self>) -> &call::CallHandle {
        if self.call.engine.is_none() {
            let (tx, rx) = async_channel::unbounded::<CallEvent>();
            let h = call::start(SERVER.into(), tx.clone());
            // New video pictures arrive on other threads: nudge the UI (at most ~30 times a second).
            let last = Arc::new(std::sync::atomic::AtomicI64::new(0));
            *h.frames.wake.lock().unwrap() = Some(Box::new(move || {
                let now = now_ms();
                if now - last.load(std::sync::atomic::Ordering::Relaxed) >= 33 {
                    last.store(now, std::sync::atomic::Ordering::Relaxed);
                    let _ = tx.try_send(CallEvent::Changed);
                }
            }));
            h.send(Cmd::Devices { mic: self.prefs.mic_device.clone(), speaker: self.prefs.speaker_device.clone() });
            self.call.engine = Some(h);
            cx.spawn(async move |this, cx| {
                while let Ok(ev) = rx.recv().await {
                    if this.update(cx, |this, cx| this.on_call_event(ev, cx)).is_err() {
                        break;
                    }
                }
            })
            .detach();
        }
        self.call.engine.as_ref().unwrap()
    }

    pub(crate) fn call_send(&mut self, cmd: Cmd, cx: &mut Context<Self>) {
        self.engine(cx).send(cmd);
    }

    fn on_call_event(&mut self, ev: CallEvent, cx: &mut Context<Self>) {
        match ev {
            CallEvent::Changed => {}
            CallEvent::Toast(t) => self.toast(&t, 6000),
            CallEvent::Sound(_) | CallEvent::Chat(_) => {}
            CallEvent::Save(room, snap) => crate::rooms::save(&room, &snap),
        }
        let Some(h) = &self.call.engine else { return };
        self.call.view = h.view();
        // Ring the others once we're actually in the call.
        if let (Some(video), Some(key)) = (self.call.ring_when_in, self.call.dm.clone()) {
            if self.call.view.voice.as_deref() == Some(DM_VOICE) {
                self.call.ring_when_in = None;
                let to = self.not_in_call(&key);
                if let Some(store) = self.store.as_mut() {
                    store.ring(&key, &to, video);
                }
                if !to.is_empty() {
                    self.call.calling = Some(now_ms() + 30_000);
                }
                self.after_change(cx);
            }
        }
        if self.call.cam_when_in && self.call.view.voice.is_some() {
            self.call.cam_when_in = false;
            self.call_send(Cmd::Camera(true), cx);
        }
        // Stop ringing once someone else is here.
        if self.call.calling.is_some() {
            let mine = self.store.as_ref().map(|s| s.me.uid.clone()).unwrap_or_default();
            if self.call.view.in_voice(DM_VOICE).iter().any(|p| !p.uid.is_empty() && p.uid != mine) {
                self.call.calling = None;
            }
        }
        self.update_images(cx);
        cx.notify();
    }

    /// Turns the engine's newest video frames into pictures GPUI can draw.
    fn update_images(&mut self, _cx: &mut Context<Self>) {
        let Some(h) = &self.call.engine else { return };
        let keys = h.frames.keys();
        self.call.images.retain(|k, _| keys.contains(k));
        for k in keys {
            let Some(f) = h.frames.get(&k) else { continue };
            if self.call.images.get(&k).map(|(s, _)| *s) == Some(f.seq) {
                continue;
            }
            let Some(buf) = image::ImageBuffer::<image::Rgba<u8>, Vec<u8>>::from_raw(f.width, f.height, f.bgra.clone()) else { continue };
            let img = Arc::new(RenderImage::new(smallvec::smallvec![image::Frame::new(buf)]));
            if let Some((_, old)) = self.call.images.insert(k, (f.seq, img)) {
                self.old_images.push(old);
            }
        }
    }

    /// DM members who aren't in its call.
    fn not_in_call(&self, key: &str) -> Vec<String> {
        let Some(store) = &self.store else { return vec![] };
        let inside: Vec<String> = self.call.view.in_voice(DM_VOICE).iter().map(|p| p.uid.clone()).collect();
        store.dm_targets(key).into_iter().filter(|u| !inside.contains(u)).collect()
    }

    pub(crate) fn in_call_for(&self, key: &str) -> bool {
        self.call.dm.as_deref() == Some(key) && self.call.view.voice.is_some()
    }

    /// Starts or joins a DM's call, like the web app's startCall/joinDMCall.
    pub(crate) fn start_call(&mut self, key: &str, video: bool, ring: bool, cx: &mut Context<Self>) {
        let Some(store) = &self.store else { return };
        let Some(d) = store.me.dms.get(key) else { return };
        if d.left {
            return;
        }
        let room = format!("dm-{}", d.cid);
        let (uid, name, status, custom) = (store.me.uid.clone(), store.me.name.clone(), self.prefs.status.clone(), self.prefs.custom.clone());
        if self.in_call_for(key) {
            if video && !self.call.view.cam {
                self.call_send(Cmd::Camera(true), cx);
            }
            return;
        }
        self.leave_call(cx);
        let (m, df) = (self.call.muted, self.call.deaf);
        self.call_send(Cmd::Open { room, uid, name, status, custom, saved: None }, cx);
        self.call_send(Cmd::SetMuted(m), cx);
        self.call_send(Cmd::SetDeaf(df), cx);
        self.call_send(Cmd::JoinVoice(DM_VOICE.into()), cx);
        self.call.dm = Some(key.to_string());
        self.call.ring_when_in = if ring { Some(video) } else { None };
        self.call.cam_when_in = video;
        self.call.focus = None;
        cx.notify();
    }

    /// Leaves the call (and its room).
    pub(crate) fn leave_call(&mut self, cx: &mut Context<Self>) {
        if self.call.engine.is_none() {
            return;
        }
        if let (Some(_), Some(key)) = (self.call.calling.take(), self.call.dm.clone()) {
            if let Some(s) = self.store.as_mut() {
                s.ring_stop(&key);
            }
        }
        self.call.ring_when_in = None;
        self.call.cam_when_in = false;
        self.call_send(Cmd::LeaveVoice, cx);
        // A DM's call room closes with the call; a server stays open.
        if self.call.dm.take().is_some() {
            self.call_send(Cmd::Close, cx);
        }
        self.after_change(cx);
    }

    pub(crate) fn toggle_mute(&mut self, cx: &mut Context<Self>) {
        self.call.muted = !self.call.muted;
        if !self.call.muted && self.call.deaf {
            self.call.deaf = false;
            self.call_send(Cmd::SetDeaf(false), cx);
        }
        let m = self.call.muted;
        self.call_send(Cmd::SetMuted(m), cx);
        cx.notify();
    }

    pub(crate) fn toggle_deaf(&mut self, cx: &mut Context<Self>) {
        self.call.deaf = !self.call.deaf;
        let d = self.call.deaf;
        self.call_send(Cmd::SetDeaf(d), cx);
        cx.notify();
    }

    /// Someone's calling: answer (join their call room) or decline.
    pub(crate) fn answer_call(&mut self, accept: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(r) = self.store.as_mut().and_then(|s| s.ringing.take()) else { return };
        if accept {
            if let Some(s) = self.store.as_mut() {
                s.take_call_room(&r.key, &r.cid);
            }
            self.open_view(crate::app::View::Dm(r.key.clone()), window, cx);
            self.start_call(&r.key, r.video, false, cx);
        }
        cx.notify();
    }

    /// Called every second: stop ringing after 30 seconds, both ways.
    pub(crate) fn call_tick(&mut self) -> bool {
        let now = now_ms();
        let mut dirty = false;
        if self.call.calling.is_some_and(|u| u < now) {
            self.call.calling = None;
            dirty = true;
        }
        if let Some(s) = self.store.as_mut() {
            if s.ringing.as_ref().is_some_and(|r| r.until < now) {
                s.ringing = None;
                dirty = true;
            }
            // Already in that call (joined from another place): no need to ring.
            let in_room = self.call.view.room.clone();
            if s.ringing.as_ref().is_some_and(|r| in_room.as_deref() == Some(format!("dm-{}", r.cid).as_str()) && self.call.view.voice.is_some()) {
                s.ringing = None;
                dirty = true;
            }
        }
        dirty | self.call.view.voice.is_some() // the call timer
    }

    // ---------- drawing ----------

    fn tile(&self, p: &PersonView, kind: &'static str, w: f32, h: f32, cx: &mut Context<Self>) -> Stateful<Div> {
        let v = &self.call.view;
        let me = p.id == v.my_id;
        let key = format!("{}:{}", p.id, kind);
        let speaking = kind == "user" && v.speaking.contains(&p.id) && !p.state.muted;
        let img = self.video_for(p, kind);
        let color = hue(if p.uid.is_empty() { &p.id } else { &p.uid });
        let mut t = div()
            .id(SharedString::from(format!("tile-{key}")))
            .relative()
            .w(u(w))
            .h(u(h))
            .flex_none()
            .rounded(u(8.))
            .overflow_hidden()
            .bg(hsla(color / 360., 0.28, 0.22, 1.))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .border(u(3.))
            .border_color(if speaking { c_green() } else { gpui_kit::transparent_black() });
        if let Some(img) = img {
            t = t.child(div().absolute().inset_0().bg(gpui_kit::black()).flex().items_center().justify_center().child(gpui_kit::img(ImageSource::Render(img)).size_full().object_fit(ObjectFit::Contain)));
        } else {
            t = t.child(avatar(if p.uid.is_empty() { &p.id } else { &p.uid }, &p.name, (h * 0.36).min(80.)));
            if kind == "screen" {
                t = t.child(div().absolute().left_0().right_0().bottom(u(h * 0.2)).flex().justify_center().text_size(u(13.)).text_color(rgb(0xb5bac1)).child(if me { "Starting your stream…" } else { "Loading stream…" }));
            }
        }
        if kind == "screen" {
            t = t.child(div().absolute().top(u(8.)).left(u(8.)).px(u(6.)).rounded(u(4.)).bg(c_red()).text_color(white()).text_size(u(11.)).font_weight(FontWeight::BOLD).child("LIVE"));
        }
        let label = if kind == "screen" { if me { "Your stream".to_string() } else { p.name.clone() } } else { p.name.clone() };
        t = t.child(
            div().absolute().left(u(8.)).bottom(u(8.)).max_w(u(w - 16.)).flex().items_center().gap(u(6.)).px(u(8.)).py(u(3.)).rounded(u(6.))
                .bg(hsla(0., 0., 0., 0.67)).text_color(rgb(0xf2f3f5)).text_size(u(13.)).font_weight(FontWeight::SEMIBOLD)
                .when(kind == "screen", |d| d.child(call_icon("screen").size(u(16.)).text_color(white())))
                .child(div().truncate().child(label))
                .when(kind == "user" && (p.state.deaf || p.state.muted), |d| {
                    d.child(call_icon(if p.state.deaf { "headOff" } else { "micOff" }).size(u(16.)).text_color(rgb(0xf23f42)))
                }),
        );
        let k2 = key.clone();
        t.on_click(cx.listener(move |this, _, _, cx| {
            this.call.focus = if this.call.focus.as_deref() == Some(k2.as_str()) { None } else { Some(k2.clone()) };
            cx.notify();
        }))
    }

    fn video_for(&self, p: &PersonView, kind: &str) -> Option<Arc<RenderImage>> {
        let v = &self.call.view;
        let tag = if kind == "screen" { "screen" } else { "cam" };
        if p.id == v.my_id {
            return self.call.images.get(&format!("{}:{tag}", p.id)).map(|(_, i)| i.clone());
        }
        // Others' videos are keyed "<peer id>:<track id>"; the engine says which is which.
        let prefix = format!("{}:", p.id);
        self.call.images.iter()
            .find(|(k, _)| k.starts_with(&prefix) && v.video_kinds.get(*k).copied() == Some(tag))
            .map(|(_, (_, i))| i.clone())
    }

    fn ring_tile(&self, uid: &str, name: &str, w: f32, h: f32, cx: &mut Context<Self>) -> Stateful<Div> {
        let ringing = self.call.calling.is_some();
        div()
            .id(SharedString::from(format!("ring-{uid}")))
            .relative()
            .w(u(w))
            .h(u(h))
            .flex_none()
            .rounded(u(8.))
            .bg(hsla(hue(uid) / 360., 0.22, 0.18, 1.))
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(u(8.))
            .cursor_pointer()
            .opacity(if ringing { 1. } else { 0.75 })
            .child(avatar(uid, name, (h * 0.36).min(80.)))
            .child(div().text_size(u(13.)).text_color(rgb(0xb5bac1)).child(if ringing { "Ringing…" } else { "Not in the call" }))
            .child(div().absolute().left(u(8.)).bottom(u(8.)).px(u(8.)).py(u(3.)).rounded(u(6.)).bg(hsla(0., 0., 0., 0.67)).text_color(rgb(0xf2f3f5)).text_size(u(13.)).font_weight(FontWeight::SEMIBOLD).child(name.to_string()))
            .on_click(cx.listener(|this, _, _, cx| {
                if this.call.calling.is_none() {
                    if let Some(key) = this.call.dm.clone() {
                        let to = this.not_in_call(&key);
                        if let Some(s) = this.store.as_mut() {
                            s.ring(&key, &to, false);
                        }
                        this.call.calling = Some(now_ms() + 30_000);
                        this.after_change(cx);
                    }
                }
            }))
    }

    /// The call area at the top of a DM (web: `.callslot.dm`), or the whole view for a server
    /// voice channel.
    pub(crate) fn render_call_area(&self, ch: &str, dm_key: Option<&str>, height: Option<f32>, cx: &mut Context<Self>) -> Div {
        let v = &self.call.view;
        let here = v.voice.as_deref() == Some(ch);
        let inside = v.in_voice(ch);
        let store = self.store.as_ref();
        let mut tiles: Vec<AnyElement> = Vec::new();
        let (w, h) = if dm_key.is_some() { (280., 158.) } else { (360., 202.) };
        let mut items: Vec<(String, &PersonView, &'static str)> = Vec::new();
        for p in &inside {
            items.push((format!("{}:user", p.id), p, "user"));
            if p.state.screen.is_some() {
                items.push((format!("{}:screen", p.id), p, "screen"));
            }
        }
        let focus = self.call.focus.clone().filter(|f| items.iter().any(|(k, _, _)| k == f));
        let body: AnyElement = if inside.is_empty() && !here {
            let title = dm_key.and_then(|k| store.map(|s| s.dm_title(k))).unwrap_or_else(|| self.channel_name(ch));
            div().flex_1().flex().flex_col().items_center().justify_center().gap(u(4.)).text_color(rgb(0xb5bac1))
                .child(div().text_size(u(20.)).font_weight(FontWeight::BOLD).text_color(rgb(0xf2f3f5)).child(title))
                .child(if !v.ready && v.room.is_some() { "Connecting to the call…" } else if dm_key.is_some() { "No one's in the call yet." } else { "No one's here yet. Join and invite your friends!" })
                .into_any_element()
        } else if let Some(f) = focus {
            let (_, p, kind) = items.iter().find(|(k, _, _)| *k == f).unwrap();
            let big = self.tile(p, kind, if dm_key.is_some() { 560. } else { 880. }, if dm_key.is_some() { 260. } else { 495. }, cx);
            let strip: Vec<AnyElement> = items.iter().filter(|(k, _, _)| *k != f).map(|(_, p, kind)| self.tile(p, kind, 160., 90., cx).into_any_element()).collect();
            div().flex_1().min_h(px(0.)).flex().flex_col().items_center().justify_center().gap(u(8.)).p(u(16.))
                .child(big).child(div().flex().gap(u(8.)).justify_center().children(strip)).into_any_element()
        } else {
            for (_, p, kind) in &items {
                tiles.push(self.tile(p, kind, w, h, cx).into_any_element());
            }
            if let (Some(key), true, Some(s)) = (dm_key, here, store) {
                for u in s.dm_targets(key) {
                    if !inside.iter().any(|p| p.uid == u) {
                        tiles.push(self.ring_tile(&u, &s.name_of(&u), w, h, cx).into_any_element());
                    }
                }
            }
            div().id("cgrid").flex_1().min_h(px(0.)).overflow_y_scroll().flex().flex_wrap().content_center().justify_center().items_center().gap(u(8.)).p(u(16.)).children(tiles).into_any_element()
        };
        let mut area = div().flex().flex_col().min_h(px(0.)).bg(if self.prefs.theme == "light" { rgb(0x111214).into() } else { gpui_kit::black() }).text_color(rgb(0xf2f3f5));
        area = match height {
            Some(hh) => area.h(u(hh)).flex_none().border_b_1().border_color(c_border()),
            None => area.flex_1(),
        };
        area.child(body).child(self.call_bar(ch, here, dm_key.is_some(), cx))
    }

    fn channel_name(&self, ch: &str) -> String {
        self.call.view.channels.iter().find(|c| c.id == ch).map(|c| c.name.clone()).unwrap_or_else(|| "Voice".into())
    }

    fn cbtn(id: &str, icon: &str, tip: &'static str, on: bool, bad: bool) -> Stateful<Div> {
        div()
            .id(SharedString::from(id.to_string()))
            .h(u(46.))
            .min_w(u(46.))
            .px(u(10.))
            .rounded(u(10.))
            .flex()
            .items_center()
            .justify_center()
            .cursor_pointer()
            .bg(if on { c_green() } else if bad { rgb(0x3a1d20).into() } else { gpui_kit::transparent_black() })
            .text_color(if bad && !on { rgb(0xf23f42).into() } else { white() })
            .hover(|d| d.bg(if on { c_green() } else { hsla(0., 0., 1., 0.08) }))
            .tooltip(move |window, cx| Tooltip::new(tip).build(window, cx))
            .child(call_icon(icon).size(u(24.)))
    }

    fn call_bar(&self, ch: &str, here: bool, dm: bool, cx: &mut Context<Self>) -> Div {
        let bar = div().flex_none().flex().items_center().justify_center().gap(u(10.)).px(u(16.)).pt(u(10.)).pb(u(18.));
        if !here {
            let ch1 = ch.to_string();
            let ch2 = ch.to_string();
            return bar
                .child(
                    div().id("cjoin").px(u(28.)).py(u(12.)).rounded(u(10.)).bg(c_green()).text_color(white()).font_weight(FontWeight::BOLD).text_size(u(16.)).cursor_pointer()
                        .child(if dm { "Join Call" } else { "Join Voice" })
                        .on_click(cx.listener(move |this, _, _, cx| this.join_here(&ch1, false, cx))),
                )
                .when(dm, |b| {
                    b.child(
                        div().id("cjoinv").flex().items_center().gap(u(8.)).px(u(28.)).py(u(12.)).rounded(u(10.)).bg(hsla(0., 0., 1., 0.1)).text_color(white()).font_weight(FontWeight::BOLD).text_size(u(16.)).cursor_pointer()
                            .child(call_icon("camera").size(u(20.))).child("Join with Video")
                            .on_click(cx.listener(move |this, _, _, cx| this.join_here(&ch2, true, cx))),
                    )
                });
        }
        let v = &self.call.view;
        let mic_bad = self.call.muted || self.call.deaf;
        let grp = || div().flex().gap(u(4.)).bg(rgb(0x1a1b1e)).border_1().border_color(rgb(0x2b2c31)).rounded(u(14.)).p(u(6.));
        bar.child(
            grp()
                .child(Self::cbtn("cmic", if mic_bad { "micOff" } else { "mic" }, if self.call.muted { "Unmute" } else { "Mute" }, false, mic_bad)
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_mute(cx))))
                .child(Self::cbtn("cdeaf", if self.call.deaf { "headOff" } else { "head" }, if self.call.deaf { "Undeafen" } else { "Deafen" }, false, self.call.deaf)
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_deaf(cx))))
                .child(Self::cbtn("ccam", if v.cam { "camera" } else { "cameraOff" }, if v.cam { "Turn Off Camera" } else { "Turn On Camera" }, v.cam, false)
                    .on_click(cx.listener(|this, _, _, cx| {
                        let on = !this.call.view.cam;
                        this.call_send(Cmd::Camera(on), cx);
                    }))),
        )
        .child(
            grp().child(Self::cbtn("cshare", "share", if v.screen { "Stop Streaming" } else { "Share Your Screen" }, v.screen, false)
                .on_click(cx.listener(|this, _, _, cx| {
                    let cmd = if this.call.view.screen { Cmd::Screen(None) } else { Cmd::Screen(Some(None)) };
                    this.call_send(cmd, cx);
                }))),
        )
        .child(
            div().id("chang").h(u(58.)).w(u(96.)).rounded(u(14.)).bg(c_red()).text_color(white()).flex().items_center().justify_center().cursor_pointer()
                .hover(|d| d.opacity(0.9))
                .tooltip(|window, cx| Tooltip::new("Disconnect").build(window, cx))
                .child(call_icon("hangup").size(u(28.)))
                .on_click(cx.listener(|this, _, _, cx| this.leave_call(cx))),
        )
    }

    fn join_here(&mut self, ch: &str, video: bool, cx: &mut Context<Self>) {
        if ch == DM_VOICE {
            if let Some(key) = self.call.dm.clone() {
                self.call_send(Cmd::JoinVoice(DM_VOICE.into()), cx);
                self.call.cam_when_in = video;
                let _ = key;
            }
        } else {
            self.join_server_voice(ch, cx);
        }
        cx.notify();
    }

    /// "Voice Connected" above the user bar while you're in a call.
    pub(crate) fn render_voice_panel(&self, cx: &mut Context<Self>) -> Option<Div> {
        let v = &self.call.view;
        let voice = v.voice.clone()?;
        let place = match (&self.call.dm, &self.store) {
            (Some(k), Some(s)) => s.dm_title(k),
            _ => format!("{} / {}", self.channel_name(&voice), v.room.clone().unwrap_or_default()),
        };
        let secs = v.since.map(|s| s.elapsed().as_secs()).unwrap_or(0);
        let timer = format!("{}:{:02}:{:02}", secs / 3600, secs / 60 % 60, secs % 60);
        let state = if v.bad { "Connection Problem" } else if !v.connected { "Reconnecting…" } else { "Voice Connected" };
        let color = if v.bad || !v.connected { c_red() } else { c_green() };
        let vb = |id: &'static str, icon: &'static str, tip: &'static str, on: bool| {
            div().id(id).flex_1().h(u(32.)).rounded(u(6.)).flex().items_center().justify_center().cursor_pointer()
                .bg(if on { c_green() } else { c_hover() }).text_color(if on { white() } else { c_text() })
                .hover(|d| d.opacity(0.85))
                .tooltip(move |window, cx| Tooltip::new(tip).build(window, cx))
                .child(call_icon(icon).size(u(18.)))
        };
        Some(
            div().flex().flex_col().gap(u(8.)).px(u(8.)).pt(u(10.)).pb(u(8.)).border_b_1().border_color(c_border())
                .child(
                    div().flex().items_center().gap(u(6.))
                        .child(call_icon("signal").size(u(18.)).text_color(color))
                        .child(
                            div().flex_1().min_w(px(0.)).flex().flex_col()
                                .child(div().text_sm().font_weight(FontWeight::BOLD).text_color(color).child(state))
                                .child(
                                    div().id("vpname").flex().gap(u(6.)).text_xs().text_color(c_muted()).cursor_pointer().hover(|d| d.text_color(c_strong()))
                                        .child(div().truncate().child(place)).child(timer)
                                        .on_click(cx.listener(|this, _, window, cx| this.open_call_view(window, cx))),
                                ),
                        )
                        .child(
                            div().id("vpleave").size(u(32.)).rounded(u(6.)).flex().items_center().justify_center().cursor_pointer().text_color(c_text())
                                .hover(|d| d.bg(c_hover()).text_color(c_red()))
                                .tooltip(|window, cx| Tooltip::new("Disconnect").build(window, cx))
                                .child(call_icon("hangup").size(u(20.)))
                                .on_click(cx.listener(|this, _, _, cx| this.leave_call(cx))),
                        ),
                )
                .child(
                    div().flex().gap(u(8.))
                        .child(vb("vpcam", if v.cam { "camera" } else { "cameraOff" }, if v.cam { "Turn Off Camera" } else { "Turn On Camera" }, v.cam)
                            .on_click(cx.listener(|this, _, _, cx| {
                                let on = !this.call.view.cam;
                                this.call_send(Cmd::Camera(on), cx);
                            })))
                        .child(vb("vpshare", "share", if v.screen { "Stop Streaming" } else { "Share Your Screen" }, v.screen)
                            .on_click(cx.listener(|this, _, _, cx| {
                                let cmd = if this.call.view.screen { Cmd::Screen(None) } else { Cmd::Screen(Some(None)) };
                                this.call_send(cmd, cx);
                            }))),
                ),
        )
    }

    fn open_call_view(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(k) = self.call.dm.clone() {
            self.open_view(crate::app::View::Dm(k), window, cx);
        } else {
            self.open_server_call(window, cx);
        }
    }

    /// The mic and headphone buttons in the user bar (they work in and out of calls).
    pub(crate) fn user_bar_buttons(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let b = |id: &'static str, icon: &'static str, tip: &'static str, bad: bool| {
            div().id(id).size(u(32.)).rounded(u(6.)).flex().items_center().justify_center().cursor_pointer()
                .text_color(if bad { rgb(0xf23f42).into() } else { c_muted() })
                .when(bad, |d| d.bg(hsla(358. / 360., 0.75, 0.59, 0.12)))
                .hover(|d| d.bg(c_hover()).text_color(if bad { rgb(0xf23f42).into() } else { c_strong() }))
                .tooltip(move |window, cx| Tooltip::new(tip).build(window, cx))
                .child(call_icon(icon).size(u(20.)))
        };
        let mic_bad = self.call.muted || self.call.deaf;
        vec![
            b("ubmic", if mic_bad { "micOff" } else { "mic" }, if self.call.muted { "Unmute" } else { "Mute" }, mic_bad)
                .on_click(cx.listener(|this, _, _, cx| this.toggle_mute(cx)))
                .into_any_element(),
            b("ubdeaf", if self.call.deaf { "headOff" } else { "head" }, if self.call.deaf { "Undeafen" } else { "Deafen" }, self.call.deaf)
                .on_click(cx.listener(|this, _, _, cx| this.toggle_deaf(cx)))
                .into_any_element(),
        ]
    }

    /// The incoming call card (web: `.ringcard`).
    pub(crate) fn render_ring_card(&self, cx: &mut Context<Self>) -> Option<Div> {
        let s = self.store.as_ref()?;
        let r = s.ringing.as_ref()?;
        let d = s.me.dms.get(&r.key)?;
        let group = d.group;
        let title = if group { s.dm_title(&r.key) } else { r.name.clone() };
        let sub = if group { format!("{} is calling the group…", r.name) } else if r.video { "Incoming video call…".into() } else { "Incoming call…".into() };
        let rbtn = |id: &'static str, icon: &'static str, tip: &'static str, yes: bool| {
            div().id(id).size(u(48.)).rounded_full().flex().items_center().justify_center().cursor_pointer().text_color(white())
                .bg(if yes { c_green() } else { c_red() }).hover(|d| d.opacity(0.9))
                .tooltip(move |window, cx| Tooltip::new(tip).build(window, cx))
                .child(call_icon(icon).size(u(22.)))
        };
        let av_id = if group { r.key.clone() } else { r.from.clone() };
        Some(
            div().absolute().top(u(80.)).left_0().right_0().flex().justify_center().child(
                div().w(u(250.)).px(u(16.)).pt(u(22.)).pb(u(18.)).rounded(u(12.)).bg(c_float()).shadow_lg().border_1().border_color(c_border())
                    .flex().flex_col().items_center().gap(u(6.))
                    .child(avatar(&av_id, &title, 80.))
                    .child(div().text_size(u(18.)).font_weight(FontWeight::BOLD).text_color(c_strong()).child(title))
                    .child(div().text_xs().text_color(c_muted()).child(sub))
                    .child(
                        div().flex().gap(u(28.)).mt(u(10.))
                            .child(rbtn("rno", "hangup", "Decline", false).on_click(cx.listener(|this, _, window, cx| this.answer_call(false, window, cx))))
                            .child(rbtn("ryes", if r.video { "camera" } else { "phone" }, if r.video { "Join with Video" } else { "Join Call" }, true)
                                .on_click(cx.listener(|this, _, window, cx| this.answer_call(true, window, cx)))),
                    ),
            ),
        )
    }

    /// The phone and camera buttons in a DM's header.
    pub(crate) fn dm_call_buttons(&self, key: &str, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let mk = |id: &'static str, icon: &'static str, tip: &'static str| {
            div().id(id).size(u(32.)).rounded(u(6.)).flex().items_center().justify_center().cursor_pointer().text_color(c_muted())
                .hover(|d| d.bg(c_hover()).text_color(c_strong()))
                .tooltip(move |window, cx| Tooltip::new(tip).build(window, cx))
                .child(call_icon(icon).size(u(20.)))
        };
        let (k1, k2) = (key.to_string(), key.to_string());
        vec![
            mk("dmcall", "phone", "Start Voice Call").on_click(cx.listener(move |this, _, _, cx| this.start_call(&k1, false, true, cx))).into_any_element(),
            mk("dmvideo", "camera", "Start Video Call").on_click(cx.listener(move |this, _, _, cx| this.start_call(&k2, true, true, cx))).into_any_element(),
        ]
    }
}

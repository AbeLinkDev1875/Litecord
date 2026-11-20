//! Settings › Voice & Video, like the web app's: devices, volumes, Mic Test with a level meter,
//! Input Profile (Voice Isolation, Studio, Custom), voice activity or push to talk, sensitivity,
//! noise suppression, echo cancellation, gain control, camera and screen share quality, and
//! what a stream sends as sound. Also push to talk itself.

use std::collections::HashMap;

use gpui_kit::component::button::*;
use gpui_kit::component::slider::{Slider, SliderEvent, SliderState};
use gpui_kit::component::switch::Switch;
use gpui_kit::*;
use gpui_kit::prelude::FluentBuilder as _;

use crate::app::LiteCord;
use crate::prefs::*;
use crate::settings::{chip, h2, note, radio, section, toggle_row};

#[derive(Default)]
pub struct VoiceUi {
    /// A Mic Test: its own microphone and speakers, playing you back to yourself.
    test: Option<crate::call::audio::Audio>,
    sliders: HashMap<&'static str, (Entity<SliderState>, Subscription)>,
    /// Waiting for the push-to-talk key to be pressed.
    recording: bool,
    /// The push-to-talk key is held.
    ptt_down: bool,
    cameras: Option<Vec<String>>,
    sounds: Option<Vec<(String, String)>>,
}

/// A readable name for a push-to-talk key.
pub fn key_label(k: &str) -> String {
    match k {
        "" => "Not set".into(),
        "mouse4" => "Mouse 4 (Back)".into(),
        "mouse5" => "Mouse 5 (Forward)".into(),
        "space" => "Space".into(),
        k if k.len() == 1 => k.to_uppercase(),
        k => {
            let mut c = k.chars();
            c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
        }
    }
}

/// Windows' virtual-key code for a key name, so push to talk works while you're in a game.
#[cfg(windows)]
fn vk(k: &str) -> Option<i32> {
    let b = k.as_bytes();
    Some(match k {
        "mouse4" => 0x05,
        "mouse5" => 0x06,
        "space" => 0x20,
        "tab" => 0x09,
        "`" => 0xC0,
        "-" => 0xBD,
        "=" => 0xBB,
        "[" => 0xDB,
        "]" => 0xDD,
        "\\" => 0xDC,
        ";" => 0xBA,
        "'" => 0xDE,
        "," => 0xBC,
        "." => 0xBE,
        "/" => 0xBF,
        "insert" => 0x2D,
        "delete" => 0x2E,
        "home" => 0x24,
        "end" => 0x23,
        "pageup" => 0x21,
        "pagedown" => 0x22,
        _ if b.len() == 1 && b[0].is_ascii_lowercase() => (b[0] - b'a') as i32 + 0x41,
        _ if b.len() == 1 && b[0].is_ascii_digit() => (b[0] - b'0') as i32 + 0x30,
        _ if k.starts_with('f') => {
            let n: i32 = k[1..].parse().ok()?;
            if !(1..=24).contains(&n) {
                return None;
            }
            0x70 + n - 1
        }
        _ => return None,
    })
}

#[cfg(windows)]
#[link(name = "user32")]
extern "system" {
    fn GetAsyncKeyState(key: i32) -> i16;
}

fn meter(level: f32, threshold: f32, open: bool, show_threshold: bool) -> Div {
    let pos = |db: f32| ((db + 80.) / 80.).clamp(0., 1.);
    div()
        .relative()
        .h(u(10.))
        .w_full()
        .rounded(u(5.))
        .bg(c_input())
        .border_1()
        .border_color(c_border())
        .child(div().absolute().left_0().top_0().bottom_0().rounded(u(5.)).w(relative(pos(level))).bg(if open { rgb(0x23a55a) } else { rgb(0x80848e) }))
        .when(show_threshold, |d| d.child(div().absolute().top(u(-3.)).bottom(u(-3.)).w(u(3.)).rounded(u(2.)).bg(c_strong()).left(relative(pos(threshold)))))
}

impl LiteCord {
    /// Settings changed: save them and tell the call and the Mic Test.
    fn voice_changed(&mut self, cx: &mut Context<Self>) {
        self.save_prefs();
        let v = self.prefs.voice_opts();
        if let Some(e) = &self.call.engine {
            e.send(crate::call::Cmd::Voice(v.clone()));
        }
        if let Some(t) = &self.voice.test {
            t.apply(&v);
        }
        cx.notify();
    }

    fn video_changed(&mut self, cx: &mut Context<Self>) {
        self.save_prefs();
        if let Some(e) = &self.call.engine {
            e.send(self.prefs.share_cmd());
            e.send(self.prefs.cam_cmd());
        }
        cx.notify();
    }

    fn slider(
        &mut self,
        key: &'static str,
        (min, max, step): (f32, f32, f32),
        value: f32,
        cx: &mut Context<Self>,
        on: impl Fn(&mut LiteCord, f32) + 'static,
    ) -> Entity<SliderState> {
        if let Some((e, _)) = self.voice.sliders.get(key) {
            return e.clone();
        }
        let st = cx.new(|_| SliderState::new().min(min).max(max).step(step).default_value(value));
        let sub = cx.subscribe(&st, move |this, _, ev: &SliderEvent, cx| {
            let (SliderEvent::Change(v) | SliderEvent::Release(v)) = ev;
            on(this, v.start());
            if matches!(ev, SliderEvent::Release(_)) {
                this.voice_changed(cx);
            } else {
                // Apply while dragging; save when let go.
                let v = this.prefs.voice_opts();
                if let Some(e) = &this.call.engine {
                    e.send(crate::call::Cmd::Voice(v.clone()));
                }
                if let Some(t) = &this.voice.test {
                    t.apply(&v);
                }
                cx.notify();
            }
        });
        self.voice.sliders.insert(key, (st.clone(), sub));
        st
    }

    fn toggle_mic_test(&mut self, cx: &mut Context<Self>) {
        if self.voice.test.take().is_some() {
            cx.notify();
            return;
        }
        let a = crate::call::audio::Audio::new();
        a.apply(&self.prefs.voice_opts());
        a.start_test(&self.prefs.mic_device, &self.prefs.speaker_device);
        self.voice.test = Some(a);
        // Keep the meter moving while the test runs.
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(std::time::Duration::from_millis(50)).await;
            let going = this.update(cx, |this, cx| {
                let on = this.voice.test.is_some() && this.settings == Some(crate::settings::Page::Voice);
                if !on {
                    this.voice.test = None;
                } else {
                    cx.notify();
                }
                on
            });
            if !matches!(going, Ok(true)) {
                break;
            }
        })
        .detach();
        cx.notify();
    }

    /// A key or side mouse button went down or up anywhere in the window. Returns true if it
    /// was used (setting the push-to-talk key).
    pub(crate) fn voice_key(&mut self, key: &str, down: bool, cx: &mut Context<Self>) -> bool {
        if self.voice.recording && down {
            self.voice.recording = false;
            if key != "escape" {
                self.prefs.ptt_key = key.to_string();
            }
            self.voice_changed(cx);
            return true;
        }
        if self.prefs.ptt_on() && key == self.prefs.ptt_key {
            self.set_ptt(down, cx);
        }
        false
    }

    fn set_ptt(&mut self, down: bool, cx: &mut Context<Self>) {
        if self.voice.ptt_down == down {
            return;
        }
        self.voice.ptt_down = down;
        if let Some(e) = &self.call.engine {
            e.send(crate::call::Cmd::Ptt(down));
        }
        if let Some(t) = &self.voice.test {
            t.set_ptt(down);
        }
        cx.notify();
    }

    /// On Windows, push to talk also works while another window (a game) is in front.
    pub(crate) fn start_global_ptt(&mut self, cx: &mut Context<Self>) {
        #[cfg(windows)]
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(std::time::Duration::from_millis(15)).await;
            let ok = this.update(cx, |this, cx| {
                if !this.prefs.ptt_on() || this.voice.recording {
                    return;
                }
                if let Some(code) = vk(&this.prefs.ptt_key) {
                    let down = unsafe { GetAsyncKeyState(code) } as u16 & 0x8000 != 0;
                    this.set_ptt(down, cx);
                }
            });
            if ok.is_err() {
                break;
            }
        })
        .detach();
        #[cfg(not(windows))]
        let _ = cx;
    }

    pub(crate) fn page_voice(&mut self, cx: &mut Context<Self>) -> Div {
        if self.audio_devices.is_none() {
            self.audio_devices = Some(crate::call::audio::Audio::devices());
        }
        let (ins, outs) = self.audio_devices.clone().unwrap_or_default();
        // ALSA lists many plugin devices; keep the ones people pick.
        let useful = |n: &String| {
            !cfg!(target_os = "linux")
                || ["pipewire", "pulse"].contains(&n.as_str())
                || n.starts_with("sysdefault:") || n.starts_with("front:") || n.starts_with("hdmi:")
        };
        let list = |id: &'static str, names: Vec<String>, cur: String, which: u8, cx: &mut Context<Self>| {
            let mut col = div().flex().flex_col().gap(u(2.));
            let mut all = vec![String::new()];
            all.extend(names.into_iter().filter(|n| which == 2 || useful(n)));
            for (i, n) in all.into_iter().enumerate() {
                let on = n == cur;
                let label = if n.is_empty() { "Default".to_string() } else { n.clone() };
                col = col.child(
                    div().id(SharedString::from(format!("{id}-{i}"))).h(u(36.)).px(u(10.)).rounded(u(6.)).flex().items_center().gap(u(10.)).cursor_pointer()
                        .bg(if on { c_active() } else { transparent_black() }).hover(|d| d.bg(c_hover()))
                        .child(div().size(u(16.)).rounded_full().border_2().border_color(if on { c_accent() } else { c_muted() }).flex().items_center().justify_center()
                            .when(on, |d| d.child(div().size(u(8.)).rounded_full().bg(c_accent()))))
                        .child(div().truncate().text_color(c_text()).child(label))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            match which {
                                0 => this.prefs.mic_device = n.clone(),
                                1 => this.prefs.speaker_device = n.clone(),
                                _ => this.prefs.cam_device = n.clone(),
                            }
                            this.save_prefs();
                            if let Some(e) = &this.call.engine {
                                e.send(crate::call::Cmd::Devices { mic: this.prefs.mic_device.clone(), speaker: this.prefs.speaker_device.clone() });
                                e.send(this.prefs.cam_cmd());
                            }
                            if which < 2 && this.voice.test.is_some() {
                                // Restart the Mic Test on the new device.
                                this.voice.test = None;
                                this.toggle_mic_test(cx);
                            }
                            cx.notify();
                        })),
                );
            }
            col
        };
        let p = self.prefs.clone();
        let label = |t: String| div().font_weight(FontWeight::MEDIUM).text_color(c_strong()).child(t);

        // ---- Voice ----
        let in_vol = self.slider("in_vol", (0., 200., 5.), (p.in_vol * 100.).round(), cx, |this, v| this.prefs.in_vol = v / 100.);
        let out_vol = self.slider("out_vol", (0., 200., 5.), (p.out_vol * 100.).round(), cx, |this, v| this.prefs.out_vol = v / 100.);
        let testing = self.voice.test.is_some();
        let (level, thr, open) = self.voice.test.as_ref().map(|t| t.meter()).unwrap_or((-100., p.sens, false));
        let voice = section("VOICE")
            .child(div().flex().gap(u(20.))
                .child(div().flex_1().min_w(px(0.)).flex().flex_col().gap(u(6.)).child(label("Input Device".into())).child(list("mic", ins, p.mic_device.clone(), 0, cx)))
                .child(div().flex_1().min_w(px(0.)).flex().flex_col().gap(u(6.)).child(label("Output Device".into())).child(list("spk", outs, p.speaker_device.clone(), 1, cx))))
            .child(div().flex().gap(u(20.))
                .child(div().flex_1().flex().flex_col().gap(u(6.)).child(label(format!("Input Volume: {}%", (p.in_vol * 100.).round()))).child(Slider::new(&in_vol)))
                .child(div().flex_1().flex().flex_col().gap(u(6.)).child(label(format!("Output Volume: {}%", (p.out_vol * 100.).round()))).child(Slider::new(&out_vol))))
            .child(div().flex().items_center().gap(u(12.))
                .child(Button::new("mictest").primary().label(if testing { "Stop Testing" } else { "Mic Test" }).on_click(cx.listener(|this, _, _, cx| this.toggle_mic_test(cx))))
                .child(div().flex_1().child(meter(level, thr, open, testing))))
            .child(note("Start a Mic Test and say something: you'll hear yourself the way friends do. The bar turns green when your voice is being sent."));

        // ---- Input profile ----
        let prof = |id: &'static str, value: &'static str, title: &'static str, desc: &'static str, cx: &mut Context<Self>| {
            radio(id, p.input_profile == value, title, desc).on_click(cx.listener(move |this, _, _, cx| {
                this.prefs.input_profile = value.into();
                this.voice_changed(cx);
            }))
        };
        let profile = section("INPUT PROFILE")
            .child(prof("ipiso", "isolation", "Voice Isolation", "Just your voice: AI noise suppression cuts through the noise.", cx))
            .child(prof("ipstudio", "studio", "Studio", "Pure audio: open mic with no processing.", cx))
            .child(prof("ipcustom", "custom", "Custom", "Advanced mode: all the buttons and dials.", cx));

        let mut custom = div();
        if p.input_profile == "custom" {
            let mode = |id: &'static str, value: &'static str, title: &'static str, cx: &mut Context<Self>| {
                radio(id, p.input_mode == value, title, "").on_click(cx.listener(move |this, _, _, cx| {
                    this.prefs.input_mode = value.into();
                    this.voice_changed(cx);
                }))
            };
            let mut m = section("INPUT MODE").child(mode("imvad", "vad", "Voice Activity", cx)).child(mode("imptt", "ptt", "Push to Talk", cx));
            if p.input_mode == "ptt" {
                let delay = self.slider("ptt_delay", (0., 2000., 20.), p.ptt_delay as f32, cx, |this, v| this.prefs.ptt_delay = v as u32);
                let rec = self.voice.recording;
                m = m
                    .child(div().flex().gap(u(20.))
                        .child(div().flex_1().flex().flex_col().gap(u(6.)).child(label("Shortcut".into()))
                            .child(Button::new("pttkey").label(if rec { "Press a key or side mouse button… (Esc to cancel)".to_string() } else { key_label(&p.ptt_key) })
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.voice.recording = !this.voice.recording;
                                    cx.notify();
                                }))))
                        .child(div().flex_1().flex().flex_col().gap(u(6.)).child(label(format!("Push to Talk Release Delay: {} ms", p.ptt_delay))).child(Slider::new(&delay))))
                    .child(note(if cfg!(windows) {
                        "Hold your key to talk. It works even while you're in a game."
                    } else {
                        "Hold your key to talk. On Linux, apps only see keys while their window is in front, so it won't work while you're in a game. Voice Activity works everywhere."
                    }))
                    .when(self.voice.ptt_down, |d| d.child(div().text_sm().text_color(rgb(0x23a55a)).child("Talking…")));
            } else {
                let sens = self.slider("sens", (-80., 0., 1.), p.sens, cx, |this, v| this.prefs.sens = v);
                m = m
                    .child(toggle_row("Automatically determine input sensitivity", "", Switch::new("autosens").checked(p.auto_sens).on_click(cx.listener(|this, on: &bool, _, cx| {
                        this.prefs.auto_sens = *on;
                        this.voice_changed(cx);
                    }))))
                    .when(!p.auto_sens, |d| d.child(label(format!("Input Sensitivity: {} dB", p.sens.round()))).child(Slider::new(&sens)))
                    .child(meter(level, thr, open, testing))
                    .child(note("The bar turns green when your voice is loud enough to be sent. Start a Mic Test to see it move."));
            }
            let ns = |id: &'static str, value: &'static str, title: &'static str, desc: &'static str, cx: &mut Context<Self>| {
                radio(id, p.ns_mode == value, title, desc).on_click(cx.listener(move |this, _, _, cx| {
                    this.prefs.ns_mode = value.into();
                    this.voice_changed(cx);
                }))
            };
            let proc_ = section("VOICE PROCESSING")
                .child(label("Noise Suppression".into()))
                .child(ns("nsai", "ai", "AI (RNNoise)", "Removes keyboards, fans, dogs and background talk. Recommended.", cx))
                .child(ns("nsstd", "standard", "Standard", "WebRTC's built-in noise filter, like a browser's.", cx))
                .child(ns("nsoff", "off", "None", "", cx))
                .child(toggle_row("Echo Cancellation", "Stops friends from hearing themselves through your microphone.", Switch::new("ec").checked(p.ec).on_click(cx.listener(|this, on: &bool, _, cx| {
                    this.prefs.ec = *on;
                    this.voice_changed(cx);
                }))))
                .child(toggle_row("Automatic Gain Control", "Evens out your volume when you get closer or farther from the mic.", Switch::new("agc").checked(p.agc).on_click(cx.listener(|this, on: &bool, _, cx| {
                    this.prefs.agc = *on;
                    this.voice_changed(cx);
                }))));
            custom = custom.child(m).child(proc_);
        }

        // ---- Video ----
        if self.voice.cameras.is_none() {
            self.voice.cameras = Some(crate::call::video::cameras());
        }
        let cams = self.voice.cameras.clone().unwrap_or_default();
        let seg = |prefix: &'static str, items: Vec<(f32, String)>, cur: f32, set: fn(&mut Prefs, f32), cx: &mut Context<Self>| {
            div().flex().flex_wrap().gap(u(8.)).children(items.into_iter().enumerate().map(|(i, (v, l))| {
                chip(SharedString::from(format!("{prefix}{i}")), l, (cur - v).abs() < 0.001).on_click(cx.listener(move |this, _, _, cx| {
                    set(&mut this.prefs, v);
                    this.video_changed(cx);
                }))
            }))
        };
        let mbps = |v: &[f32]| v.iter().map(|m| (*m, if *m == 0. { "Auto".to_string() } else { format!("{m} Mbps") })).collect::<Vec<_>>();
        let video = section("VIDEO")
            .child(label("Camera".into()))
            .child(if cams.is_empty() { note("No camera found.").into_any_element() } else { list("cam", cams, p.cam_device.clone(), 2, cx).into_any_element() })
            .child(label("Camera Resolution".into()))
            .child(seg("camres", vec![(360., "360p (lightest)".into()), (720., "720p".into()), (1080., "1080p".into())], p.cam_res as f32, |p, v| p.cam_res = v as u32, cx))
            .child(label("Camera Frame Rate".into()))
            .child(seg("camfps", vec![(15., "15 FPS".into()), (30., "30 FPS".into()), (60., "60 FPS".into())], p.cam_fps as f32, |p, v| p.cam_fps = v as u32, cx))
            .child(label("Camera Bitrate".into()))
            .child(seg("cambr", mbps(&[0., 0.5, 1., 2., 4., 8.]), p.cam_mbps, |p, v| p.cam_mbps = v, cx))
            .child(note("Changes apply right away, even while your camera is on. On a slow computer or connection, pick 360p at 15 FPS."));

        // ---- Screen share ----
        if self.voice.sounds.is_none() {
            self.voice.sounds = Some(crate::call::share_audio::choices());
        }
        let mut sounds = self.voice.sounds.clone().unwrap_or_default();
        if !sounds.iter().any(|(v, _)| *v == p.share_sound) {
            if let Some(app) = p.share_sound.strip_prefix("app:") {
                sounds.push((p.share_sound.clone(), format!("{app} (not playing right now)")));
            }
        }
        let mut sound_list = div().flex().flex_col().gap(u(2.));
        for (i, (v, l)) in sounds.into_iter().enumerate() {
            let on = v == p.share_sound;
            sound_list = sound_list.child(
                div().id(SharedString::from(format!("ssnd-{i}"))).h(u(36.)).px(u(10.)).rounded(u(6.)).flex().items_center().gap(u(10.)).cursor_pointer()
                    .bg(if on { c_active() } else { transparent_black() }).hover(|d| d.bg(c_hover()))
                    .child(div().size(u(16.)).rounded_full().border_2().border_color(if on { c_accent() } else { c_muted() }).flex().items_center().justify_center()
                        .when(on, |d| d.child(div().size(u(8.)).rounded_full().bg(c_accent()))))
                    .child(div().truncate().text_color(c_text()).child(l))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.prefs.share_sound = v.clone();
                        this.video_changed(cx);
                    })),
            );
        }
        let share = section("SCREEN SHARE")
            .child(label("Resolution".into()))
            .child(seg("shres", vec![(720., "720p".into()), (1080., "1080p".into()), (1440., "1440p".into()), (0., "Source".into())], p.share_res as f32, |p, v| p.share_res = v as u32, cx))
            .child(label("Frame Rate".into()))
            .child(seg("shfps", vec![(15., "15 FPS".into()), (30., "30 FPS".into()), (60., "60 FPS".into())], p.share_fps as f32, |p, v| p.share_fps = v as u32, cx))
            .child(label("Bitrate".into()))
            .child(seg("shbr", mbps(&[0., 2., 4., 6., 8., 12., 20.]), p.share_mbps, |p, v| p.share_mbps = v, cx))
            .child(note("Quality changes apply right away, even while you're live. More bitrate looks cleaner but your upload has to keep up."))
            .child(label("Stream Sound".into()))
            .child(sound_list)
            .child(note(if cfg!(target_os = "linux") {
                "\"All apps\" sends everything your computer plays except LiteCord, so friends never hear themselves. Pick one app to send only its sound. Apps that aren't playing yet show up after you click Look for Devices Again. Applies the next time you go live."
            } else {
                "\"All apps\" sends everything your computer plays except LiteCord, so friends never hear themselves. Applies the next time you go live."
            }));

        div()
            .child(h2("Voice & Video"))
            .child(voice)
            .child(profile)
            .child(custom)
            .child(video)
            .child(share)
            .child(
                section("").child(Button::new("rescan").label("Look for Devices Again").on_click(cx.listener(|this, _, _, cx| {
                    this.audio_devices = None;
                    this.voice.cameras = None;
                    this.voice.sounds = None;
                    cx.notify();
                }))),
            )
    }
}

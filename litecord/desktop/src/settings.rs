//! User Settings, laid out like Discord's (and the web app's): a sidebar with a pinned search
//! box and sections split by lines, the page in the middle, and a close button with "ESC" under it.

use gpui_kit::component::button::*;
use gpui_kit::component::input::Input;
use gpui_kit::component::switch::Switch;
use gpui_kit::component::*;
use gpui_kit::*;
use gpui_kit::{prelude::FluentBuilder as _, white};

use crate::app::{avatar, hue, LiteCord, SERVER};
use crate::net;
use crate::prefs::*;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Page {
    Account,
    Appearance,
    Window,
    Notifications,
    Keybinds,
    Language,
    Advanced,
}

impl Page {
    pub fn from_name(s: &str) -> Option<Page> {
        Some(match s {
            "account" => Page::Account,
            "appearance" => Page::Appearance,
            "window" => Page::Window,
            "notifications" => Page::Notifications,
            "keybinds" => Page::Keybinds,
            "language" => Page::Language,
            "advanced" => Page::Advanced,
            _ => return None,
        })
    }
}

struct Item {
    page: Page,
    label: &'static str,
    icon: IconName,
    /// Extra words the search box matches.
    words: &'static str,
}

fn window_label() -> &'static str {
    if cfg!(target_os = "windows") {
        "Windows Settings"
    } else if cfg!(target_os = "macos") {
        "Window"
    } else {
        "Linux Settings"
    }
}

fn sections() -> Vec<(&'static str, Vec<Item>)> {
    vec![
        ("User Settings", vec![Item { page: Page::Account, label: "My Account", icon: IconName::User, words: "name username friend code profile" }]),
        (
            "App Settings",
            vec![
                Item { page: Page::Appearance, label: "Appearance", icon: IconName::Palette, words: "theme dark light ash onyx zoom scale size font compact cozy" },
                Item { page: Page::Window, label: window_label(), icon: IconName::Frame, words: "title bar border frame decorations minimize maximize close buttons native" },
                Item { page: Page::Notifications, label: "Notifications", icon: IconName::Bell, words: "desktop alerts preview" },
                Item { page: Page::Keybinds, label: "Keybinds", icon: IconName::SquareTerminal, words: "shortcuts keys keyboard" },
                Item { page: Page::Language, label: "Language & Time", icon: IconName::Globe, words: "clock 12 24 hour time" },
                Item { page: Page::Advanced, label: "Advanced", icon: IconName::Settings, words: "server data folder version" },
            ],
        ),
    ]
}

/// The zoom we'd suggest for a screen this tall (in the desktop's own pixels).
fn suggested_zoom(screen_h: f32) -> u32 {
    match screen_h as u32 {
        h if h >= 2000 => 150,
        h if h >= 1400 => 125,
        h if h >= 1000 => 100,
        h if h >= 850 => 90,
        _ => 80,
    }
}

fn h2(t: impl Into<SharedString>) -> Div {
    div().text_xl().font_weight(FontWeight::BOLD).text_color(c_strong()).mb(u(20.)).child(t.into())
}

/// A titled block, with a line under it like Discord's sections.
fn section(title: &'static str) -> Div {
    div()
        .pb(u(20.))
        .mb(u(20.))
        .border_b_1()
        .border_color(c_border())
        .flex()
        .flex_col()
        .gap(u(10.))
        .when(!title.is_empty(), |d| d.child(div().text_xs().font_weight(FontWeight::BOLD).text_color(c_muted()).child(title)))
}

fn note(t: impl Into<SharedString>) -> Div {
    div().text_sm().text_color(c_muted()).child(t.into())
}

fn chip(id: impl Into<ElementId>, label: impl Into<SharedString>, on: bool) -> Stateful<Div> {
    div()
        .id(id)
        .px(u(10.))
        .h(u(30.))
        .rounded(u(6.))
        .flex()
        .items_center()
        .justify_center()
        .text_sm()
        .font_weight(FontWeight::MEDIUM)
        .cursor_pointer()
        .border_1()
        .when(on, |d| d.bg(c_accent()).border_color(c_accent()).text_color(white()))
        .when(!on, |d| d.bg(c_input()).border_color(c_border()).text_color(c_text()).hover(|d| d.bg(c_hover())))
        .child(label.into())
}

/// A radio row (the circle, a title and a description), like Discord's.
fn radio(id: impl Into<ElementId>, on: bool, title: impl Into<SharedString>, desc: impl Into<SharedString>) -> Stateful<Div> {
    let desc: SharedString = desc.into();
    div()
        .id(id)
        .px(u(12.))
        .py(u(10.))
        .rounded(u(6.))
        .flex()
        .items_center()
        .gap(u(12.))
        .cursor_pointer()
        .bg(if on { c_active() } else { c_input() })
        .hover(|d| d.bg(c_hover()))
        .child(
            div()
                .flex_none()
                .size(u(20.))
                .rounded_full()
                .border(u(2.))
                .border_color(if on { c_strong() } else { c_muted() })
                .flex()
                .items_center()
                .justify_center()
                .when(on, |d| d.child(div().size(u(10.)).rounded_full().bg(c_strong()))),
        )
        .child(
            div()
                .flex_1()
                .flex()
                .flex_col()
                .child(div().font_weight(FontWeight::MEDIUM).text_color(c_strong()).child(title.into()))
                .when(!desc.is_empty(), |d| d.child(div().text_sm().text_color(c_muted()).child(desc))),
        )
}

/// A switch row: title and description on the left, the switch on the right.
fn toggle_row(title: &'static str, desc: &'static str, switch: Switch) -> Div {
    div()
        .flex()
        .items_center()
        .gap(u(16.))
        .child(
            div()
                .flex_1()
                .flex()
                .flex_col()
                .gap(u(2.))
                .child(div().font_weight(FontWeight::MEDIUM).text_color(c_strong()).child(title))
                .when(!desc.is_empty(), |d| d.child(note(desc))),
        )
        .child(switch)
}

impl LiteCord {
    pub(crate) fn render_settings(&mut self, page: Page, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let query = self.settings_search.read(cx).value().to_lowercase();
        let mut list = div().id("setlist").flex_1().min_h(px(0.)).overflow_y_scroll().flex().flex_col().pr(u(6.)).pb(u(20.));
        for (i, (title, items)) in sections().into_iter().enumerate() {
            let items: Vec<Item> = items
                .into_iter()
                .filter(|it| query.is_empty() || it.label.to_lowercase().contains(&query) || it.words.contains(query.as_str()))
                .collect();
            if items.is_empty() {
                continue;
            }
            if i > 0 {
                list = list.child(div().mx(u(10.)).my(u(8.)).h(px(1.)).bg(c_border()));
            }
            let _ = title;
            for it in items {
                let on = it.page == page;
                let p = it.page;
                list = list.child(
                    div()
                        .id(SharedString::from(format!("set-{:?}", it.page)))
                        .h(u(44.))
                        .px(u(12.))
                        .rounded(u(8.))
                        .flex()
                        .items_center()
                        .gap(u(14.))
                        .cursor_pointer()
                        .text_size(u(16.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(if on { c_strong() } else { c_muted() })
                        .when(on, |d| d.bg(c_active()))
                        .hover(|d| d.bg(c_hover()).text_color(c_text()))
                        .child(Icon::new(it.icon).size(u(22.)))
                        .child(it.label)
                        .on_click(cx.listener(move |this, _, window, cx| this.open_settings(p, window, cx))),
                );
            }
        }
        if query.is_empty() || "log out".contains(query.as_str()) {
            list = list.child(div().mx(u(10.)).my(u(8.)).h(px(1.)).bg(c_border())).child(
                div()
                    .id("logout")
                    .h(u(44.))
                    .px(u(12.))
                    .rounded(u(8.))
                    .flex()
                    .items_center()
                    .gap(u(14.))
                    .cursor_pointer()
                    .text_size(u(16.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(c_red())
                    .hover(|d| d.bg(c_red().opacity(0.1)))
                    .child(Icon::new(IconName::ArrowLeft).size(u(22.)))
                    .child(if self.logout_armed { "Click again to log out" } else { "Log Out" })
                    .on_click(cx.listener(|this, _, _, cx| {
                        if this.logout_armed {
                            this.log_out(cx);
                        } else {
                            this.logout_armed = true;
                            cx.notify();
                        }
                    })),
            );
        }
        list = list.child(div().px(u(10.)).pt(u(12.)).text_xs().text_color(c_muted()).child(format!("LiteCord Desktop {}", env!("CARGO_PKG_VERSION"))));

        let sidebar = div()
            .w(u(300.))
            .flex_none()
            .h_full()
            .bg(c_side())
            .child(
                div()
                    .w_full()
                    .h_full()
                    .pt(u(24.))
                    .pl(u(14.))
                    .flex()
                    .flex_col()
                    .child(div().pr(u(14.)).pb(u(14.)).flex_none().child(Input::new(&self.settings_search).prefix(Icon::new(IconName::Search).size(u(14.)).text_color(c_muted())).cleanable(true)))
                    .child(list),
            );

        let body = match page {
            Page::Account => self.page_account(cx),
            Page::Appearance => self.page_appearance(window, cx),
            Page::Window => self.page_window(window, cx),
            Page::Notifications => self.page_notifications(cx),
            Page::Keybinds => self.page_keybinds(),
            Page::Language => self.page_language(cx),
            Page::Advanced => self.page_advanced(),
        };
        let content = div()
            .flex_1()
            .min_w(px(0.))
            .max_w(u(800.))
            .h_full()
            .flex()
            .child(div().id("setpage").flex_1().min_w(px(0.)).h_full().overflow_y_scroll().pt(u(40.)).px(u(56.)).pb(u(80.)).child(body))
            .child(
                div().w(u(72.)).pt(u(40.)).flex_none().flex().flex_col().items_center().gap(u(6.)).child(
                    div()
                        .id("closeset")
                        .size(u(36.))
                        .rounded_full()
                        .border(u(2.))
                        .border_color(c_muted())
                        .text_color(c_muted())
                        .flex()
                        .items_center()
                        .justify_center()
                        .cursor_pointer()
                        .hover(|d| d.bg(c_hover()).text_color(c_strong()).border_color(c_strong()))
                        .child(Icon::new(IconName::Close).size(u(18.)))
                        .on_click(cx.listener(|this, _, window, cx| this.close_settings(window, cx))),
                )
                .child(div().text_xs().font_weight(FontWeight::BOLD).text_color(c_muted()).child("ESC")),
            );
        div().size_full().flex().bg(c_main()).child(sidebar).child(div().flex_1().flex().bg(c_main()).child(content))
    }

    fn page_account(&mut self, cx: &mut Context<Self>) -> Div {
        let Some(store) = &self.store else { return div() };
        let (uid, name, username) = (store.me.uid.clone(), store.me.name.clone(), store.me.username.clone());
        let card = div()
            .rounded(u(8.))
            .overflow_hidden()
            .bg(c_panel())
            .mb(u(24.))
            .child(div().h(u(100.)).bg(hsla(hue(&uid) / 360., 0.45, 0.40, 1.)))
            .child(
                div()
                    .px(u(16.))
                    .pb(u(16.))
                    .flex()
                    .items_end()
                    .gap(u(16.))
                    .child(div().mt(u(-40.)).rounded_full().border(u(6.)).border_color(c_panel()).child(avatar(&uid, &name, 80.)))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .child(div().text_xl().font_weight(FontWeight::BOLD).text_color(c_strong()).child(name.clone()))
                            .child(note(if username.is_empty() { "No username yet".to_string() } else { username.clone() })),
                    ),
            );
        let msg = self.account_msg.clone();
        div()
            .child(h2("My Account"))
            .child(card)
            .child(
                section("DISPLAY NAME").child(note("This is how you appear to friends and in DMs.")).child(
                    div()
                        .flex()
                        .gap(u(8.))
                        .child(div().flex_1().child(Input::new(&self.rename_input)))
                        .child(Button::new("savename").primary().label("Save").on_click(cx.listener(|this, _, _, cx| {
                            let n = this.rename_input.read(cx).value().to_string();
                            if let Some(s) = this.store.as_mut() {
                                s.rename(&n);
                            }
                            this.account_msg = Some((true, "Display name saved.".into()));
                            this.after_change(cx);
                        }))),
                ),
            )
            .child(
                section("USERNAME")
                    .child(note("Optional. Friends can add you by your username instead of your friend code. First come, first served."))
                    .child(
                        div().flex().gap(u(8.)).child(div().flex_1().child(Input::new(&self.username_input))).child(Button::new("saveuser").primary().label("Save").on_click(cx.listener(
                            |this, _, window, cx| {
                                let Some(s) = &this.store else { return };
                                let (uid, key) = (s.me.uid.clone(), s.me.key.clone());
                                let wanted = this.username_input.read(cx).value().trim().trim_start_matches('@').to_lowercase();
                                this.account_msg = Some((true, "Saving…".into()));
                                cx.notify();
                                let w2 = wanted.clone();
                                let task = cx.background_executor().spawn(async move { net::set_username(SERVER, &uid, &key, &w2) });
                                cx.spawn_in(window, async move |this, cx| {
                                    let res = task.await;
                                    let _ = this.update(cx, |this, cx| {
                                        match res {
                                            Ok(()) => {
                                                if let Some(s) = this.store.as_mut() {
                                                    s.me.username = wanted.clone();
                                                    s.save();
                                                }
                                                this.account_msg = Some((true, if wanted.is_empty() { "Username removed.".into() } else { format!("Your username is now {wanted}.") }));
                                            }
                                            Err(e) => this.account_msg = Some((false, e)),
                                        }
                                        cx.notify();
                                    });
                                })
                                .detach();
                            },
                        ))),
                    ),
            )
            .children(msg.map(|(ok, t)| div().mt(u(-10.)).mb(u(16.)).text_sm().text_color(if ok { c_green() } else { c_red() }).child(t)))
            .child(
                section("FRIEND CODE").child(note("Anyone with this code can send you a friend request.")).child(
                    div()
                        .flex()
                        .items_center()
                        .gap(u(8.))
                        .child(div().flex_1().px(u(12.)).py(u(8.)).rounded(u(6.)).bg(c_input()).font_family("monospace").text_color(c_strong()).child(uid.clone()))
                        .child(Button::new("copycode").label("Copy").on_click(cx.listener(move |this, _, _, cx| {
                            cx.write_to_clipboard(ClipboardItem::new_string(uid.clone()));
                            this.toast("Friend code copied.", 2000);
                            cx.notify();
                        }))),
                ),
            )
    }

    fn page_appearance(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let mut themes = div().flex().gap(u(16.));
        for (key, label) in THEMES {
            let on = self.prefs.theme == key;
            let p = palette_for(key);
            themes = themes.child(
                div()
                    .id(SharedString::from(format!("theme-{key}")))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(u(6.))
                    .cursor_pointer()
                    .child(
                        div()
                            .size(u(60.))
                            .rounded_full()
                            .bg(rgb(p.main))
                            .border(u(if on { 3. } else { 1. }))
                            .border_color(if on { c_accent() } else { c_border() })
                            .flex()
                            .items_center()
                            .justify_center()
                            .child(div().size(u(24.)).rounded_full().bg(rgb(p.side)).border_1().border_color(rgb(p.border))),
                    )
                    .child(div().text_sm().text_color(if on { c_strong() } else { c_muted() }).child(label))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.prefs.theme = key.to_string();
                        this.save_prefs();
                        this.apply_look(cx);
                    })),
            );
        }
        let zoom = self.prefs.zoom;
        let mut zooms = div().flex().flex_wrap().gap(u(6.));
        for z in ZOOM_STEPS {
            zooms = zooms.child(chip(SharedString::from(format!("z{z}")), format!("{z}%"), z == zoom).on_click(cx.listener(move |this, _, _, cx| this.set_zoom(z, cx))));
        }
        let screen = window.display(cx).map(|d| d.bounds().size);
        let suggest = screen.map(|s| (suggested_zoom(f32::from(s.height)), s));
        let font = self.prefs.chat_font;
        let mut fonts = div().flex().flex_wrap().gap(u(6.));
        for f in [12u32, 14, 15, 16, 18, 20, 24] {
            fonts = fonts.child(chip(SharedString::from(format!("f{f}")), format!("{f}px"), f == font).on_click(cx.listener(move |this, _, _, cx| {
                this.prefs.chat_font = f;
                this.save_prefs();
                this.apply_look(cx);
            })));
        }
        // A little preview, like Discord's, so you can see the font size and layout before going back.
        let me = self.store.as_ref().map(|s| (s.me.uid.clone(), s.me.name.clone())).unwrap_or_default();
        let preview_line = |name: &str, uid: &str, text: &str| {
            let name_color = hsla(hue(uid) / 360., 0.6, if pal().light { 0.35 } else { 0.75 }, 1.);
            if self.prefs.compact {
                div()
                    .flex()
                    .gap(u(8.))
                    .text_size(u(font as f32))
                    .child(div().text_xs().text_color(c_muted()).pt(u(3.)).child("12:00"))
                    .child(div().font_weight(FontWeight::MEDIUM).text_color(name_color).child(name.to_string()))
                    .child(div().text_color(c_text()).child(text.to_string()))
            } else {
                div()
                    .flex()
                    .gap(u(14.))
                    .child(avatar(uid, name, 40.))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .child(div().flex().items_baseline().gap(u(8.)).child(div().text_size(u(font as f32)).font_weight(FontWeight::MEDIUM).text_color(name_color).child(name.to_string())).child(div().text_xs().text_color(c_muted()).child("Today at 12:00")))
                            .child(div().text_size(u(font as f32)).text_color(c_text()).child(text.to_string())),
                    )
            }
        };
        let preview = div()
            .p(u(16.))
            .rounded(u(8.))
            .bg(c_main())
            .border_1()
            .border_color(c_border())
            .flex()
            .flex_col()
            .gap(u(12.))
            .child(preview_line("Wumpie", "aaaaaaaaaaaaaaab", "Look at me, I'm a beautiful butterfly 🦋"))
            .child(preview_line(if me.1.is_empty() { "You" } else { &me.1 }, if me.0.is_empty() { "aaaaaaaaaaaaaaac" } else { &me.0 }, "Fluttering in the moonlight 🌙 日本語"));
        div()
            .child(h2("Appearance"))
            .child(div().mb(u(20.)).child(preview))
            .child(section("THEME").child(themes))
            .child(
                section("ZOOM LEVEL")
                    .child(note("Makes everything in LiteCord bigger or smaller. Ctrl + and Ctrl − change it from anywhere, and Ctrl 0 resets it."))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(u(10.))
                            .child(Button::new("zminus").icon(IconName::Minus).on_click(cx.listener(|this, _, _, cx| this.set_zoom(zoom_step(this.prefs.zoom, -1), cx))))
                            .child(div().w(u(64.)).text_center().text_lg().font_weight(FontWeight::BOLD).text_color(c_strong()).child(format!("{zoom}%")))
                            .child(Button::new("zplus").icon(IconName::Plus).on_click(cx.listener(|this, _, _, cx| this.set_zoom(zoom_step(this.prefs.zoom, 1), cx))))
                            .child(Button::new("zreset").ghost().label("Reset").on_click(cx.listener(|this, _, _, cx| this.set_zoom(100, cx)))),
                    )
                    .child(zooms)
                    .children(suggest.map(|(z, s)| {
                        div()
                            .flex()
                            .items_center()
                            .gap(u(10.))
                            .child(note(format!("Suggested for this screen ({}×{}): {z}%", f32::from(s.width) as u32, f32::from(s.height) as u32)))
                            .when(z != zoom, |d| d.child(Button::new("zsuggest").small().label(format!("Use {z}%")).on_click(cx.listener(move |this, _, _, cx| this.set_zoom(z, cx)))))
                    })),
            )
            .child(section("CHAT FONT SCALING").child(note("The size of messages and the message box.")).child(fonts))
            .child(
                section("MESSAGE DISPLAY")
                    .child(radio("cozy", !self.prefs.compact, "Cozy", "Modern, beautiful, and easy on your eyes.").on_click(cx.listener(|this, _, _, cx| {
                        this.prefs.compact = false;
                        this.save_prefs();
                        this.apply_look(cx);
                    })))
                    .child(radio("compact", self.prefs.compact, "Compact", "Fit more messages on screen at one time.").on_click(cx.listener(|this, _, _, cx| {
                        this.prefs.compact = true;
                        this.save_prefs();
                        this.apply_look(cx);
                    }))),
            )
    }

    fn page_window(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Div {
        let custom = !self.prefs.native_frame;
        let names = |v: &Vec<Ctl>| {
            v.iter()
                .map(|c| match c {
                    Ctl::Min => "Minimize",
                    Ctl::Max => "Maximize",
                    Ctl::Close => "Close",
                })
                .collect::<Vec<_>>()
                .join(", ")
        };
        let (dl, dr) = self.detected.clone();
        let detected = match (dl.is_empty(), dr.is_empty()) {
            (true, true) => "none".to_string(),
            (true, false) => format!("{} (on the right)", names(&dr)),
            (false, true) => format!("{} (on the left)", names(&dl)),
            (false, false) => format!("{} on the left, {} on the right", names(&dl), names(&dr)),
        };
        let restart_needed = cfg!(target_os = "windows") && self.prefs.native_frame != self.native_at_start;
        // A preview of the title bar with the buttons that would show.
        let (pl, pr) = self.window_buttons(window);
        let mini = |list: &Vec<Ctl>| {
            div().flex().children(list.iter().map(|c| {
                let icon = match c {
                    Ctl::Min => IconName::WindowMinimize,
                    Ctl::Max => IconName::WindowMaximize,
                    Ctl::Close => IconName::WindowClose,
                };
                div().w(u(36.)).h(u(28.)).flex().items_center().justify_center().text_color(c_muted()).child(Icon::new(icon).size(u(12.)))
            }))
        };
        let preview = div()
            .rounded(u(8.))
            .overflow_hidden()
            .border_1()
            .border_color(c_border())
            .child(
                div()
                    .h(u(28.))
                    .bg(c_rail())
                    .flex()
                    .items_center()
                    .child(mini(&pl))
                    .child(div().flex_1().text_center().text_xs().font_weight(FontWeight::SEMIBOLD).text_color(c_text()).child("LiteCord"))
                    .child(mini(&pr)),
            )
            .child(div().h(u(40.)).bg(c_main()));
        let manual = !self.prefs.buttons_auto;
        div()
            .child(h2(window_label()))
            .child(
                section("WINDOW FRAME")
                    .child(toggle_row(
                        "Use LiteCord's title bar",
                        "Draws the title bar and border in LiteCord's theme colors, like Discord. Turn it off to use your system's title bar instead.",
                        Switch::new("frame").checked(custom).on_click(cx.listener(|this, on: &bool, window, cx| {
                            this.prefs.native_frame = !*on;
                            this.save_prefs();
                            if cfg!(target_os = "linux") {
                                window.request_decorations(if *on { WindowDecorations::Client } else { WindowDecorations::Server });
                            }
                            cx.notify();
                        })),
                    ))
                    .when(restart_needed, |d| {
                        d.child(
                            div().flex().items_center().gap(u(10.)).child(note("Restart LiteCord to switch the window frame.")).child(
                                Button::new("restart").primary().small().label("Restart Now").on_click(|_, _, cx| {
                                    if let Ok(exe) = std::env::current_exe() {
                                        let _ = std::process::Command::new(exe).spawn();
                                    }
                                    cx.quit();
                                }),
                            ),
                        )
                    }),
            )
            .child(
                section("WINDOW BUTTONS")
                    .when(!custom, |d| d.opacity(0.5))
                    .child(note("Which buttons LiteCord's title bar shows."))
                    .child(preview)
                    .child(radio("btnauto", !manual, "Match my desktop", format!("Detected: {detected}")).on_click(cx.listener(|this, _, _, cx| {
                        this.prefs.buttons_auto = true;
                        this.save_prefs();
                        cx.notify();
                    })))
                    .child(radio("btnmanual", manual, "Choose my own", "Pick the buttons below.").on_click(cx.listener(|this, _, _, cx| {
                        this.prefs.buttons_auto = false;
                        this.save_prefs();
                        cx.notify();
                    })))
                    .when(manual, |d| {
                        d.child(toggle_row("Minimize", "", Switch::new("bmin").checked(self.prefs.btn_min).on_click(cx.listener(|this, on: &bool, _, cx| {
                            this.prefs.btn_min = *on;
                            this.save_prefs();
                            cx.notify();
                        }))))
                        .child(toggle_row("Maximize", "", Switch::new("bmax").checked(self.prefs.btn_max).on_click(cx.listener(|this, on: &bool, _, cx| {
                            this.prefs.btn_max = *on;
                            this.save_prefs();
                            cx.notify();
                        }))))
                        .child(toggle_row("Close", "You can always close with Alt+F4 too.", Switch::new("bclose").checked(self.prefs.btn_close).on_click(cx.listener(|this, on: &bool, _, cx| {
                            this.prefs.btn_close = *on;
                            this.save_prefs();
                            cx.notify();
                        }))))
                    }),
            )
    }

    fn page_notifications(&mut self, cx: &mut Context<Self>) -> Div {
        div()
            .child(h2("Notifications"))
            .child(
                section("")
                    .child(toggle_row(
                        "Enable Desktop Notifications",
                        "Get a notification when a DM arrives and you're not looking at it. Do Not Disturb turns them off.",
                        Switch::new("notify").checked(self.prefs.notify).on_click(cx.listener(|this, on: &bool, _, cx| {
                            this.prefs.notify = *on;
                            this.save_prefs();
                            cx.notify();
                        })),
                    ))
                    .child(toggle_row(
                        "Show Message Text",
                        "Turn off to only show who sent it, not what they said.",
                        Switch::new("preview").checked(self.prefs.notify_preview).on_click(cx.listener(|this, on: &bool, _, cx| {
                            this.prefs.notify_preview = *on;
                            this.save_prefs();
                            cx.notify();
                        })),
                    )),
            )
            .child(
                section("").child(
                    Button::new("testnotify").label("Send a Test Notification").on_click(|_, _, _| {
                        std::thread::spawn(|| {
                            let _ = notify_rust::Notification::new().appname("LiteCord").summary("LiteCord").body("Notifications are working! 🎉").show();
                        });
                    }),
                ),
            )
    }

    fn page_keybinds(&self) -> Div {
        let row = |keys: &'static str, what: &'static str| {
            div()
                .flex()
                .items_center()
                .py(u(10.))
                .border_b_1()
                .border_color(c_border())
                .child(div().flex_1().text_color(c_text()).child(what))
                .child(div().px(u(8.)).py(u(2.)).rounded(u(4.)).bg(c_input()).border_1().border_color(c_border()).text_sm().font_weight(FontWeight::SEMIBOLD).text_color(c_strong()).child(keys))
        };
        div()
            .child(h2("Keybinds"))
            .child(note("Shortcuts that work anywhere in LiteCord."))
            .child(
                div()
                    .mt(u(16.))
                    .child(row("Ctrl +", "Zoom in"))
                    .child(row("Ctrl −", "Zoom out"))
                    .child(row("Ctrl 0", "Reset zoom"))
                    .child(row("Ctrl ,", "Open or close User Settings"))
                    .child(row("Esc", "Close Settings or a menu"))
                    .child(row("Enter", "Send your message")),
            )
    }

    fn page_language(&mut self, cx: &mut Context<Self>) -> Div {
        let clock = self.prefs.clock.clone();
        let auto24 = Prefs { clock: "auto".into(), ..Default::default() }.clock24();
        let auto_desc = format!("Follows your computer. Right now that's {}-hour time.", if auto24 { 24 } else { 12 });
        let pick = |id: &'static str, value: &'static str, title: &'static str, desc: String, cx: &mut Context<Self>| {
            radio(id, clock == value, title, desc).on_click(cx.listener(move |this, _, _, cx| {
                this.prefs.clock = value.to_string();
                this.save_prefs();
                this.apply_look(cx);
            }))
        };
        div()
            .child(h2("Language & Time"))
            .child(section("LANGUAGE").child(note("LiteCord is in English for now.")))
            .child(
                section("TIME FORMAT")
                    .child(pick("clkauto", "auto", "Automatic", auto_desc, cx))
                    .child(pick("clk12", "12", "12-hour", "1:30 PM".into(), cx))
                    .child(pick("clk24", "24", "24-hour", "13:30".into(), cx)),
            )
    }

    fn page_advanced(&mut self) -> Div {
        let dir = crate::store::data_dir();
        let dir_s = dir.display().to_string();
        div()
            .child(h2("Advanced"))
            .child(
                section("SERVER")
                    .child(note(format!("Friends and DMs go through {SERVER}. Servers and calls are in the browser version for now.")))
                    .child(Button::new("openweb").label("Open LiteCord in the Browser").on_click(|_, _, cx| cx.open_url(SERVER))),
            )
            .child(
                section("DATA FOLDER")
                    .child(note("Your account, chats and settings are saved here, on this computer only."))
                    .child(div().px(u(12.)).py(u(8.)).rounded(u(6.)).bg(c_input()).font_family("monospace").text_sm().text_color(c_strong()).child(dir_s))
                    .child(Button::new("opendata").label("Open Folder").on_click(move |_, _, cx| cx.reveal_path(&dir))),
            )
            .child(
                section("ABOUT").child(note(format!(
                    "LiteCord Desktop {}, made with Rust and GPUI (the UI engine from Zed, through GPUI Kit).",
                    env!("CARGO_PKG_VERSION")
                ))),
            )
    }
}

//! LiteCord's own title bar, like Discord's: drawn in the theme's colors, with the window
//! buttons your desktop uses (or the ones you pick in Settings › Window).

use gpui_kit::component::*;
use gpui_kit::*;
use gpui_kit::{prelude::FluentBuilder as _, white};

use crate::app::{LiteCord, View};
use crate::prefs::*;

impl LiteCord {
    /// Whether LiteCord is drawing the frame itself right now.
    pub(crate) fn custom_frame(&self, window: &Window) -> bool {
        if cfg!(target_os = "linux") {
            matches!(window.window_decorations(), Decorations::Client { .. })
        } else if cfg!(target_os = "windows") {
            !self.native_at_start
        } else {
            false // macOS keeps its own traffic lights
        }
    }

    /// The buttons to show, as (left side, right side).
    pub(crate) fn window_buttons(&self, window: &Window) -> (Vec<Ctl>, Vec<Ctl>) {
        let (left, right) = if self.prefs.buttons_auto {
            self.detected.clone()
        } else {
            let mut v = Vec::new();
            if self.prefs.btn_min {
                v.push(Ctl::Min);
            }
            if self.prefs.btn_max {
                v.push(Ctl::Max);
            }
            if self.prefs.btn_close {
                v.push(Ctl::Close);
            }
            (vec![], v)
        };
        // Some window managers (tiling ones) can't minimize or maximize at all.
        let can = window.window_controls();
        let keep = |c: &Ctl| match c {
            Ctl::Min => can.minimize,
            Ctl::Max => can.maximize,
            Ctl::Close => true,
        };
        (left.into_iter().filter(keep).collect(), right.into_iter().filter(keep).collect())
    }

    pub(crate) fn title_text(&self) -> String {
        if self.store.is_none() {
            return "LiteCord".into();
        }
        if self.settings.is_some() {
            return "User Settings".into();
        }
        match &self.view {
            View::Friends(_) => "Friends".into(),
            View::Dm(k) => self.store.as_ref().map(|s| s.dm_title(k)).unwrap_or_default(),
            View::Server(r) => r.clone(),
        }
    }

    pub(crate) fn render_title_bar(&mut self, title: &str, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        if !self.custom_frame(window) {
            return div().into_any_element();
        }
        let (left, right) = self.window_buttons(window);
        let maximized = window.is_maximized();
        let buttons = |list: Vec<Ctl>, side: &'static str| {
            div().flex().h_full().children(list.into_iter().map(move |c| ctl_button(c, side, maximized)))
        };
        div()
            .id("titlebar")
            .h(u(32.))
            .flex_none()
            .flex()
            .items_center()
            .bg(c_rail())
            .text_color(c_muted())
            .child(buttons(left, "l"))
            .child(
                div()
                    .id("drag")
                    .flex_1()
                    .h_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap(u(8.))
                    .window_control_area(WindowControlArea::Drag)
                    // Drag to move (it starts on the first mouse move, so double clicks still work).
                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, _| this.drag_armed = true))
                    .on_mouse_up(MouseButton::Left, cx.listener(|this, _, _, _| this.drag_armed = false))
                    .on_mouse_move(cx.listener(|this, _, window, _| {
                        if this.drag_armed {
                            this.drag_armed = false;
                            window.start_window_move();
                        }
                    }))
                    .on_click(|ev, window, _| {
                        if ev.click_count() >= 2 {
                            window.zoom_window();
                        }
                    })
                    .on_mouse_down(MouseButton::Right, |ev, window, _| window.show_window_menu(ev.position))
                    .child(
                        div()
                            .size(u(16.))
                            .rounded(u(5.))
                            .bg(c_accent())
                            .text_color(white())
                            .text_size(u(8.))
                            .font_weight(FontWeight::BOLD)
                            .flex()
                            .items_center()
                            .justify_center()
                            .child("LC"),
                    )
                    .child(div().text_size(u(13.)).font_weight(FontWeight::SEMIBOLD).text_color(c_text()).child(title.to_string())),
            )
            .children(self.render_update_button(cx))
            .child(buttons(right, "r"))
            .into_any_element()
    }
}

fn ctl_button(c: Ctl, side: &str, maximized: bool) -> Stateful<Div> {
    let (id, icon, area) = match c {
        Ctl::Min => ("min", IconName::WindowMinimize, WindowControlArea::Min),
        Ctl::Max => ("max", if maximized { IconName::WindowRestore } else { IconName::WindowMaximize }, WindowControlArea::Max),
        Ctl::Close => ("close", IconName::WindowClose, WindowControlArea::Close),
    };
    div()
        .id(SharedString::from(format!("{side}-{id}")))
        .w(u(44.))
        .h_full()
        .flex()
        .items_center()
        .justify_center()
        .text_color(c_muted())
        .window_control_area(area)
        .map(|d| match c {
            Ctl::Close => d.hover(|d| d.bg(c_red()).text_color(white())),
            _ => d.hover(|d| d.bg(c_hover()).text_color(c_strong())),
        })
        .child(Icon::new(icon).size(u(14.)))
        .on_click(move |_, window, cx| match c {
            Ctl::Min => window.minimize_window(),
            Ctl::Max => window.zoom_window(),
            Ctl::Close => {
                window.remove_window();
                cx.quit();
            }
        })
}

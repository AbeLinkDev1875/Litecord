//! LiteCord for the desktop, in Rust with GPUI (the UI engine from Zed, through GPUI Kit).
//! It talks to the same Cloudflare server as the web app, so friends and DMs work between them.
mod app;
mod net;
mod store;

use gpui_kit::component::theme::{Theme, ThemeMode};
use gpui_kit::*;

fn main() {
    gpui_kit::application().with_assets(gpui_kit::assets::Assets).run(move |cx| {
        gpui_kit::init(cx);
        Theme::change(ThemeMode::Dark, None, cx);
        let bounds = Bounds::centered(None, size(px(1280.), px(800.)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions { title: Some("LiteCord".into()), ..Default::default() }),
            app_id: Some("litecord".into()),
            window_min_size: Some(size(px(800.), px(500.))),
            ..Default::default()
        };
        gpui_kit::open_window(options, cx, |window, cx| cx.new(|cx| app::LiteCord::new(window, cx))).expect("Failed to open window");
        cx.activate(true);
    });
}

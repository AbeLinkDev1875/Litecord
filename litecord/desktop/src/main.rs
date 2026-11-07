//! LiteCord for the desktop, in Rust with GPUI (the UI engine from Zed, through GPUI Kit).
//! It talks to the same Cloudflare server as the web app, so friends and DMs work between them.
// No black console window next to the app on Windows.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod net;
mod prefs;
mod settings;
mod store;
mod titlebar;

use gpui_kit::*;

fn main() {
    let prefs = prefs::Prefs::load();
    gpui_kit::application().with_assets(gpui_kit::assets::Assets).run(move |cx| {
        gpui_kit::init(cx);
        app::bind_keys(cx);
        let size = size(px(prefs.win_w.max(800.)), px(prefs.win_h.max(500.)));
        let bounds = Bounds::centered(None, size, cx);
        let custom = !prefs.native_frame;
        let options = WindowOptions {
            window_bounds: Some(if prefs.maximized { WindowBounds::Maximized(bounds) } else { WindowBounds::Windowed(bounds) }),
            // With LiteCord's own frame, the title bar is ours to draw (Windows) and the
            // border and shadow are too (Linux). Otherwise the system draws them.
            titlebar: Some(TitlebarOptions { title: Some("LiteCord".into()), appears_transparent: custom, ..Default::default() }),
            window_decorations: Some(if custom { WindowDecorations::Client } else { WindowDecorations::Server }),
            window_background: WindowBackgroundAppearance::Opaque,
            app_owns_titlebar_drag: true,
            app_id: Some("litecord".into()),
            window_min_size: Some(gpui_kit::size(px(800.), px(500.))),
            ..Default::default()
        };
        gpui_kit::open_window(options, cx, |window, cx| cx.new(|cx| app::LiteCord::new(prefs.clone(), window, cx))).expect("Failed to open window");
        cx.activate(true);
    });
}

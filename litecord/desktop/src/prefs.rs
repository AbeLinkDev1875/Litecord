//! Settings that belong to this computer rather than your account (they stay when you log out),
//! the color themes, the UI scale, and which window buttons your desktop shows.

use std::cell::Cell;
use std::path::PathBuf;

use gpui_kit::{px, rgb, Hsla, Pixels};
use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    /// "dark", "ash", "onyx" or "light" (the same themes as the web app).
    pub theme: String,
    /// UI scale in percent, like Discord's Zoom Level.
    pub zoom: u32,
    /// Chat text size in pixels, like Discord's Chat Font Scaling.
    pub chat_font: u32,
    /// Cozy (avatars and names on every group) or compact (one line per message).
    pub compact: bool,
    /// Use the system's title bar and border instead of LiteCord's own.
    pub native_frame: bool,
    /// true: show the same window buttons as the rest of the desktop. false: use the three below.
    pub buttons_auto: bool,
    pub btn_min: bool,
    pub btn_max: bool,
    pub btn_close: bool,
    pub notify: bool,
    pub notify_preview: bool,
    /// "auto", "12" or "24".
    pub clock: String,
    /// "online", "idle", "dnd" or "invisible".
    pub status: String,
    pub custom: String,
    pub win_w: f32,
    pub win_h: f32,
    pub maximized: bool,
}

impl Default for Prefs {
    fn default() -> Self {
        Prefs {
            theme: "dark".into(),
            zoom: 100,
            chat_font: 15,
            compact: false,
            native_frame: false,
            buttons_auto: true,
            btn_min: true,
            btn_max: true,
            btn_close: true,
            notify: true,
            notify_preview: true,
            clock: "auto".into(),
            status: "online".into(),
            custom: String::new(),
            win_w: 1280.,
            win_h: 800.,
            maximized: false,
        }
    }
}

fn prefs_path() -> PathBuf {
    crate::store::data_dir().join("prefs.json")
}

impl Prefs {
    pub fn load() -> Prefs {
        std::fs::read_to_string(prefs_path()).ok().and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default()
    }
    pub fn save(&self) {
        let p = prefs_path();
        let _ = std::fs::create_dir_all(p.parent().unwrap());
        if let Ok(s) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(p, s);
        }
    }
    pub fn clock24(&self) -> bool {
        match self.clock.as_str() {
            "24" => true,
            "12" => false,
            // Most of the world uses 24-hour time; the US, Canada, Australia and a few others use 12.
            _ => {
                let lang = std::env::var("LC_TIME").or_else(|_| std::env::var("LANG")).unwrap_or_default();
                !["en_US", "en_CA", "en_AU", "en_NZ", "en_PH", "es_MX", "hi_IN", "en_IN"].iter().any(|l| lang.starts_with(l)) && !lang.is_empty()
            }
        }
    }
}

/* ---------- UI scale ---------- */

/// Discord's zoom steps.
pub const ZOOM_STEPS: [u32; 11] = [50, 67, 75, 80, 90, 100, 110, 125, 150, 175, 200];

thread_local! {
    static SCALE: Cell<f32> = const { Cell::new(1.0) };
    static PAL: Cell<Palette> = const { Cell::new(DARK) };
}

pub fn set_scale(zoom: u32) {
    SCALE.with(|s| s.set(zoom as f32 / 100.));
}
pub fn scale() -> f32 {
    SCALE.with(|s| s.get())
}
/// A size in LiteCord's own pixels, grown or shrunk by the UI scale.
pub fn u(x: f32) -> Pixels {
    px(x * scale())
}

pub fn zoom_step(zoom: u32, dir: i32) -> u32 {
    let i = match ZOOM_STEPS.iter().position(|z| *z == zoom) {
        Some(i) => i as i32 + dir,
        None => {
            // Between two steps: go to the next one in that direction.
            let up = ZOOM_STEPS.iter().position(|z| *z > zoom).unwrap_or(ZOOM_STEPS.len()) as i32;
            if dir > 0 { up } else { up - 1 }
        }
    };
    ZOOM_STEPS[i.clamp(0, ZOOM_STEPS.len() as i32 - 1) as usize]
}

/* ---------- themes (the web app's color tokens) ---------- */

#[derive(Clone, Copy)]
pub struct Palette {
    pub light: bool,
    pub rail: u32,
    pub side: u32,
    pub main: u32,
    pub panel: u32,
    pub float: u32,
    pub input: u32,
    pub hover: u32,
    pub active: u32,
    pub msghover: u32,
    #[allow(dead_code)] // the web app's secondary-button color, for buttons still to come
    pub btn2: u32,
    pub border: u32,
    pub text: u32,
    pub strong: u32,
    pub muted: u32,
}

const DARK: Palette = Palette { light: false, rail: 0x121214, side: 0x18181b, main: 0x1d1e21, panel: 0x141416, float: 0x111113, input: 0x25262a, hover: 0x26272b, active: 0x303136, msghover: 0x212226, btn2: 0x2e2f35, border: 0x2a2b30, text: 0xdbdee1, strong: 0xf2f3f5, muted: 0x949ba4 };
const ASH: Palette = Palette { light: false, rail: 0x1e1f22, side: 0x2b2d31, main: 0x313338, panel: 0x232428, float: 0x111214, input: 0x383a40, hover: 0x35373c, active: 0x404249, msghover: 0x2e3035, btn2: 0x4e5058, border: 0x26272b, text: 0xdbdee1, strong: 0xf2f3f5, muted: 0x949ba4 };
const ONYX: Palette = Palette { light: false, rail: 0x000000, side: 0x09090a, main: 0x060607, panel: 0x0c0c0e, float: 0x141416, input: 0x16161a, hover: 0x151518, active: 0x1f1f24, msghover: 0x0d0d10, btn2: 0x232328, border: 0x1a1a1e, text: 0xdbdee1, strong: 0xf2f3f5, muted: 0x8e939b };
const LIGHT: Palette = Palette { light: true, rail: 0xe3e5e8, side: 0xf2f3f5, main: 0xffffff, panel: 0xebedef, float: 0xffffff, input: 0xebedef, hover: 0xe6e7e9, active: 0xd7d9dc, msghover: 0xf5f5f6, btn2: 0xd4d7dc, border: 0xe1e2e4, text: 0x313338, strong: 0x060607, muted: 0x5c5e66 };

pub const THEMES: [(&str, &str); 4] = [("light", "Light"), ("ash", "Ash"), ("dark", "Dark"), ("onyx", "Onyx")];

pub fn palette_for(theme: &str) -> Palette {
    match theme {
        "ash" => ASH,
        "onyx" => ONYX,
        "light" => LIGHT,
        _ => DARK,
    }
}
pub fn set_theme(theme: &str) {
    PAL.with(|p| p.set(palette_for(theme)));
}
pub fn pal() -> Palette {
    PAL.with(|p| p.get())
}

fn c(x: u32) -> Hsla {
    rgb(x).into()
}
pub fn c_rail() -> Hsla { c(pal().rail) }
pub fn c_side() -> Hsla { c(pal().side) }
pub fn c_main() -> Hsla { c(pal().main) }
pub fn c_panel() -> Hsla { c(pal().panel) }
pub fn c_float() -> Hsla { c(pal().float) }
pub fn c_input() -> Hsla { c(pal().input) }
pub fn c_hover() -> Hsla { c(pal().hover) }
pub fn c_active() -> Hsla { c(pal().active) }
pub fn c_msghover() -> Hsla { c(pal().msghover) }
pub fn c_border() -> Hsla { c(pal().border) }
pub fn c_text() -> Hsla { c(pal().text) }
pub fn c_strong() -> Hsla { c(pal().strong) }
pub fn c_muted() -> Hsla { c(pal().muted) }
pub fn c_accent() -> Hsla { c(0x5865f2) }
pub fn c_green() -> Hsla { c(0x23a55a) }
pub fn c_red() -> Hsla { c(0xda373c) }
pub fn c_yellow() -> Hsla { c(0xf0b232) }

/* ---------- window buttons ---------- */

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Ctl {
    Min,
    Max,
    Close,
}

/// Which buttons the desktop puts on title bars, and on which side: (left, right).
/// KDE keeps it in kwinrc (I = minimize, A = maximize, X = close), GNOME and most others in
/// the "button-layout" setting ("appmenu:minimize,maximize,close"). Windows and macOS show all three.
pub fn detect_buttons() -> (Vec<Ctl>, Vec<Ctl>) {
    let all = (vec![], vec![Ctl::Min, Ctl::Max, Ctl::Close]);
    if !cfg!(target_os = "linux") {
        return all;
    }
    let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default().to_uppercase();
    if desktop.contains("KDE") {
        if let Some(r) = kde_buttons() {
            return r;
        }
    }
    if let Some(r) = gnome_buttons() {
        return r;
    }
    if desktop.contains("KDE") {
        return (vec![], vec![Ctl::Min, Ctl::Max, Ctl::Close]);
    }
    all
}

fn kde_buttons() -> Option<(Vec<Ctl>, Vec<Ctl>)> {
    let cfg = dirs::config_dir()?.join("kwinrc");
    let text = std::fs::read_to_string(cfg).ok()?;
    let mut in_deco = false;
    let (mut left, mut right) = (None, None);
    for line in text.lines() {
        let line = line.trim();
        if line.starts_with('[') {
            in_deco = line == "[org.kde.kdecoration2]";
            continue;
        }
        if !in_deco {
            continue;
        }
        if let Some(v) = line.strip_prefix("ButtonsOnLeft=") {
            left = Some(v.to_string());
        } else if let Some(v) = line.strip_prefix("ButtonsOnRight=") {
            right = Some(v.to_string());
        }
    }
    // KDE's defaults when the lines are missing: menu on the left, help/min/max/close on the right.
    let conv = |s: &str| s.chars().filter_map(|c| match c { 'I' => Some(Ctl::Min), 'A' => Some(Ctl::Max), 'X' => Some(Ctl::Close), _ => None }).collect::<Vec<_>>();
    Some((conv(&left.unwrap_or_else(|| "MS".into())), conv(&right.unwrap_or_else(|| "HIAX".into()))))
}

fn gnome_buttons() -> Option<(Vec<Ctl>, Vec<Ctl>)> {
    let out = std::process::Command::new("gsettings").args(["get", "org.gnome.desktop.wm.preferences", "button-layout"]).output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(parse_button_layout(String::from_utf8_lossy(&out.stdout).trim().trim_matches('\'')))
}

pub fn parse_button_layout(s: &str) -> (Vec<Ctl>, Vec<Ctl>) {
    let conv = |side: &str| {
        side.split(',')
            .filter_map(|b| match b.trim() {
                "minimize" => Some(Ctl::Min),
                "maximize" => Some(Ctl::Max),
                "close" => Some(Ctl::Close),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    let (l, r) = s.split_once(':').unwrap_or(("", s));
    (conv(l), conv(r))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gnome_layouts() {
        assert_eq!(parse_button_layout(":minimize,close"), (vec![], vec![Ctl::Min, Ctl::Close]));
        assert_eq!(parse_button_layout("close,minimize:appmenu"), (vec![Ctl::Close, Ctl::Min], vec![]));
        assert_eq!(parse_button_layout("appmenu:minimize,maximize,close"), (vec![], vec![Ctl::Min, Ctl::Max, Ctl::Close]));
    }

    #[test]
    fn zoom_steps() {
        assert_eq!(zoom_step(100, 1), 110);
        assert_eq!(zoom_step(100, -1), 90);
        assert_eq!(zoom_step(200, 1), 200);
        assert_eq!(zoom_step(50, -1), 50);
        assert_eq!(zoom_step(120, 1), 125);
        assert_eq!(zoom_step(120, -1), 110);
    }
}

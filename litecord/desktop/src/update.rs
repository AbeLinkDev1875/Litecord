//! Updates, like Discord's: LiteCord looks for a newer release on GitHub now and then. When
//! there is one, a green download button shows at the top right; hover it and it says "Update",
//! click it and LiteCord downloads the new version, checks it against the release's
//! SHA256SUMS.txt, swaps itself out and restarts. Nothing happens unless you click.

use std::io::Read;
use std::path::{Path, PathBuf};

use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use sha2::{Digest, Sha256};

use crate::app::LiteCord;
use crate::prefs::*;

const REPO: &str = "AbeLinkDev1875/Litecord";
pub const RELEASES: &str = "https://github.com/AbeLinkDev1875/Litecord/releases/latest";

#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub version: String,
    asset: String,
    asset_url: String,
    sums_url: String,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub enum State {
    #[default]
    None,
    Available(Release),
    Working(Release),
    Failed(Release, String),
}

#[derive(Default)]
pub struct UpdateUi {
    pub state: State,
    hover: bool,
    /// When we last asked GitHub, and whether the window was just brought back to the front.
    last_check: Option<std::time::Instant>,
    pub(crate) refocused: bool,
    /// What the last manual check found, for Settings.
    pub checked: Option<String>,
}

/// "2.10.0" > "2.9.1".
pub fn newer(remote: &str, local: &str) -> bool {
    let parse = |v: &str| v.trim_start_matches('v').split('.').map(|p| p.parse::<u64>().unwrap_or(0)).collect::<Vec<_>>();
    let (r, l) = (parse(remote), parse(local));
    for i in 0..r.len().max(l.len()) {
        let (a, b) = (r.get(i).copied().unwrap_or(0), l.get(i).copied().unwrap_or(0));
        if a != b {
            return a > b;
        }
    }
    false
}

/// Our version. `LITECORD_PRETEND_VERSION` pretends to be another one, to test updating.
fn current_version() -> String {
    std::env::var("LITECORD_PRETEND_VERSION").unwrap_or_else(|_| env!("CARGO_PKG_VERSION").to_string())
}

fn asset_name(version: &str) -> Option<String> {
    if cfg!(windows) {
        Some(format!("LiteCord-{version}-windows-x86_64.exe"))
    } else if cfg!(target_os = "linux") {
        Some(format!("LiteCord-{version}-linux-x86_64.tar.gz"))
    } else {
        None
    }
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder().timeout_global(Some(std::time::Duration::from_secs(120))).build().into()
}

/// The latest release on GitHub, if it's newer than us and has a download for this system.
pub fn check() -> Result<Option<Release>, String> {
    let v: serde_json::Value = agent()
        .get(&format!("https://api.github.com/repos/{REPO}/releases/latest"))
        .header("User-Agent", "LiteCord-Desktop")
        .header("Accept", "application/vnd.github+json")
        .call()
        .map_err(|e| e.to_string())?
        .body_mut()
        .read_json()
        .map_err(|e| e.to_string())?;
    let tag = v["tag_name"].as_str().unwrap_or("");
    let version = tag.trim_start_matches('v').to_string();
    if version.is_empty() || !newer(&version, &current_version()) {
        return Ok(None);
    }
    let Some(asset) = asset_name(&version) else { return Ok(None) };
    let url = |name: &str| v["assets"].as_array().into_iter().flatten().find(|a| a["name"] == name).and_then(|a| a["browser_download_url"].as_str()).map(str::to_string);
    match (url(&asset), url("SHA256SUMS.txt")) {
        (Some(asset_url), Some(sums_url)) => Ok(Some(Release { version, asset, asset_url, sums_url })),
        _ => Ok(None), // still being published
    }
}

fn download(url: &str, limit: u64) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    agent()
        .get(url)
        .header("User-Agent", "LiteCord-Desktop")
        .call()
        .map_err(|e| e.to_string())?
        .into_body()
        .into_reader()
        .take(limit)
        .read_to_end(&mut out)
        .map_err(|e| e.to_string())?;
    Ok(out)
}

/// Downloads `r`, checks it, and puts it where this program is. Returns the program to start.
pub fn install(r: &Release) -> Result<PathBuf, String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let exe = exe.canonicalize().unwrap_or(exe);
    let data = fetch(r)?;
    let dir = exe.parent().ok_or("no folder")?.to_path_buf();
    let new_exe = unpack(&data, &dir)?;
    swap(&exe, &new_exe)?;
    Ok(exe)
}

/// Downloads a release's file for this system and checks it against SHA256SUMS.txt.
fn fetch(r: &Release) -> Result<Vec<u8>, String> {
    let sums = String::from_utf8_lossy(&download(&r.sums_url, 1 << 20)?).into_owned();
    let want = sums
        .lines()
        .find_map(|l| {
            let mut it = l.split_whitespace();
            let (h, n) = (it.next()?, it.next()?);
            (n.trim_start_matches('*') == r.asset).then(|| h.to_lowercase())
        })
        .ok_or("the release has no checksum for this download")?;
    let data = download(&r.asset_url, 500 << 20)?;
    let got: String = Sha256::digest(&data).iter().map(|b| format!("{b:02x}")).collect();
    if got != want {
        return Err("the download didn't match its checksum, so it wasn't used".into());
    }
    Ok(data)
}

/// The new program, written next to the old one.
fn unpack(data: &[u8], dir: &Path) -> Result<PathBuf, String> {
    let cant = |e: std::io::Error| format!("LiteCord can't write to {} ({e}). Download the new version from GitHub instead.", dir.display());
    let new_exe = dir.join(if cfg!(windows) { "litecord.new.exe" } else { ".litecord.new" });
    if cfg!(windows) {
        std::fs::write(&new_exe, data).map_err(cant)?;
        return Ok(new_exe);
    }
    // Linux: a .tar.gz with one file, `litecord`.
    let tmp = std::env::temp_dir().join(format!("litecord-update-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp).map_err(|e| e.to_string())?;
    let tgz = tmp.join("update.tar.gz");
    std::fs::write(&tgz, data).map_err(|e| e.to_string())?;
    let ok = std::process::Command::new("tar").arg("-xzf").arg(&tgz).arg("-C").arg(&tmp).status().map_err(|e| format!("couldn't unpack the update ({e})"))?;
    if !ok.success() {
        return Err("couldn't unpack the update".into());
    }
    std::fs::copy(tmp.join("litecord"), &new_exe).map_err(cant)?;
    let _ = std::fs::remove_dir_all(&tmp);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&new_exe, std::fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())?;
    }
    Ok(new_exe)
}

/// Puts `new_exe` where `exe` is. Windows can't overwrite a running program but can rename it,
/// so the old one moves aside (and is deleted on the next start).
fn swap(exe: &Path, new_exe: &Path) -> Result<(), String> {
    if cfg!(windows) {
        let old = exe.with_extension("old.exe");
        let _ = std::fs::remove_file(&old);
        std::fs::rename(exe, &old).map_err(|e| format!("couldn't move the old version aside ({e})"))?;
        if let Err(e) = std::fs::rename(new_exe, exe) {
            let _ = std::fs::rename(&old, exe);
            return Err(format!("couldn't put the new version in place ({e})"));
        }
    } else {
        std::fs::rename(new_exe, exe).map_err(|e| format!("couldn't put the new version in place ({e})"))?;
    }
    Ok(())
}

/// Removes what an update left behind (the old program on Windows).
pub fn cleanup() {
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::fs::remove_file(exe.with_extension("old.exe"));
        if let Some(d) = exe.parent() {
            let _ = std::fs::remove_file(d.join(if cfg!(windows) { "litecord.new.exe" } else { ".litecord.new" }));
        }
    }
}

impl LiteCord {
    /// Looks for updates soon after starting and every few hours, if that's turned on.
    /// Looks for updates soon after starting, every 30 minutes, and when you come back to the
    /// window (at most every 5 minutes), if that's turned on.
    pub(crate) fn start_update_checks(&mut self, cx: &mut Context<Self>) {
        if cfg!(debug_assertions) && std::env::var_os("LITECORD_PRETEND_VERSION").is_none() {
            return; // a build from source isn't a release
        }
        use std::time::{Duration, Instant};
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_secs(3)).await;
            loop {
                let due = this.update(cx, |this, _| {
                    let u = &mut this.update;
                    let since = u.last_check.map(|t| t.elapsed()).unwrap_or(Duration::MAX);
                    let due = this.prefs.update_check
                        && !matches!(u.state, State::Working(_))
                        && (since >= Duration::from_secs(30 * 60) || (u.refocused && since >= Duration::from_secs(5 * 60)));
                    u.refocused = false;
                    if due {
                        u.last_check = Some(Instant::now());
                    }
                    due
                });
                match due {
                    Err(_) => break,
                    Ok(true) => {
                        let found = cx.background_executor().spawn(async { check() }).await;
                        let _ = this.update(cx, |this, cx| this.found_update(found, cx));
                    }
                    Ok(false) => {}
                }
                cx.background_executor().timer(Duration::from_secs(15)).await;
            }
        })
        .detach();
    }

    /// Shows the button for a newer release (or a newer one than the button showed).
    fn found_update(&mut self, found: Result<Option<Release>, String>, cx: &mut Context<Self>) {
        let Ok(Some(r)) = found else { return };
        let show = match &self.update.state {
            State::None => true,
            State::Available(old) | State::Failed(old, _) => newer(&r.version, &old.version),
            State::Working(_) => false,
        };
        if show {
            self.update.state = State::Available(r);
            cx.notify();
        }
    }

    /// "Check for Updates" in Settings.
    pub(crate) fn check_updates_now(&mut self, cx: &mut Context<Self>) {
        self.update.checked = Some("Checking…".into());
        cx.notify();
        cx.spawn(async move |this, cx| {
            let found = cx.background_executor().spawn(async { check() }).await;
            let _ = this.update(cx, |this, cx| {
                this.update.checked = Some(match found {
                    Ok(Some(r)) => {
                        let msg = format!("Version {} is out. Click the green button at the top right to update.", r.version);
                        if !matches!(this.update.state, State::Working(_)) {
                            this.update.state = State::Available(r);
                        }
                        msg
                    }
                    Ok(None) => "You have the newest version.".into(),
                    Err(e) => format!("Couldn't check: {e}"),
                });
                cx.notify();
            });
        })
        .detach();
    }

    fn run_update(&mut self, cx: &mut Context<Self>) {
        let r = match &self.update.state {
            State::Available(r) | State::Failed(r, _) => r.clone(),
            _ => return,
        };
        self.update.state = State::Working(r.clone());
        cx.notify();
        cx.spawn(async move |this, cx| {
            let rr = r.clone();
            let res = cx.background_executor().spawn(async move { install(&rr) }).await;
            let _ = this.update(cx, |this, cx| match res {
                Ok(exe) => {
                    this.save_prefs();
                    if let Some(e) = this.call.engine.take() {
                        e.send(crate::call::Cmd::Quit);
                    }
                    match std::process::Command::new(&exe).spawn() {
                        Ok(_) => cx.quit(),
                        Err(e) => {
                            this.update.state = State::Failed(r, format!("Updated! Start LiteCord again to use it ({e})."));
                            cx.notify();
                        }
                    }
                }
                Err(e) => {
                    this.toast(&format!("Couldn't update: {e}"), 8000);
                    this.update.state = State::Failed(r, e);
                    cx.notify();
                }
            });
        })
        .detach();
    }

    /// The green button (title bar or top right of the window). Nothing when there's no update.
    pub(crate) fn render_update_button(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (version, working, failed) = match &self.update.state {
            State::None => return None,
            State::Available(r) => (r.version.clone(), false, None),
            State::Working(r) => (r.version.clone(), true, None),
            State::Failed(r, e) => (r.version.clone(), false, Some(e.clone())),
        };
        let green = rgb(0x23a55a);
        let open = self.update.hover || working;
        let label = if working { "Updating…".to_string() } else if failed.is_some() { "Try Again".into() } else { "Update".into() };
        let tip = match &failed {
            Some(e) => format!("Version {version}: {e}"),
            None => format!("Version {version} is ready. Click to update and restart LiteCord. You can keep using this one as long as you like."),
        };
        Some(
            div()
                .id("update")
                .h(u(24.))
                .px(u(6.))
                .mx(u(6.))
                .rounded(u(12.))
                .flex()
                .items_center()
                .gap(u(4.))
                .cursor_pointer()
                .bg(green.opacity(if open { 1.0 } else { 0.28 }))
                .text_color(if open { white() } else { rgb(0x2dc770).into() })
                .text_size(u(12.))
                .font_weight(FontWeight::BOLD)
                .child(crate::callui::call_icon("download").size(u(16.)))
                .when(open, |d| d.child(label))
                .on_hover(cx.listener(|this, h: &bool, _, cx| {
                    this.update.hover = *h;
                    cx.notify();
                }))
                .on_click(cx.listener(|this, _, _, cx| this.run_update(cx)))
                .tooltip(move |window, cx| gpui_kit::component::tooltip::Tooltip::new(tip.clone()).build(window, cx))
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: GPUI has its own `test` macro that would replace `#[test]`.
    use super::{asset_name, fetch, newer, unpack, Release, REPO};

    /// Downloads the real v2.2.0 release for this system, checks it and unpacks it into a
    /// scratch folder (nothing is replaced). `cargo test update::tests::real -- --ignored`
    #[test]
    #[ignore]
    fn real_release_downloads_and_checks() {
        let v = "2.2.0";
        let asset = asset_name(v).unwrap();
        let base = format!("https://github.com/{REPO}/releases/download/v{v}/");
        let r = Release { version: v.into(), asset_url: format!("{base}{asset}"), sums_url: format!("{base}SHA256SUMS.txt"), asset };
        let data = fetch(&r).unwrap();
        let dir = std::env::temp_dir().join(format!("litecord-update-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let exe = unpack(&data, &dir).unwrap();
        let len = std::fs::metadata(&exe).unwrap().len();
        println!("{} bytes checked and unpacked to {}", len, exe.display());
        assert!(len > 1_000_000);
        // A wrong checksum is refused.
        let bad = Release { sums_url: "https://github.com/AbeLinkDev1875/Litecord/releases/download/v2.1.0/SHA256SUMS.txt".into(), ..r.clone() };
        assert!(fetch(&bad).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn compares_versions() {
        assert!(newer("2.3.0", "2.2.0"));
        assert!(newer("v2.10.0", "2.9.9"));
        assert!(newer("3.0", "2.99.99"));
        assert!(!newer("2.2.0", "2.2.0"));
        assert!(!newer("2.1.9", "2.2.0"));
        assert!(!newer("", "2.2.0"));
    }
}

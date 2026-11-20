//! Stream sound: what your computer plays, sent along with your screen share (like Discord's).
//!
//! - Linux (PipeWire): every app that's playing gets its own `pw-record` pointed at it, and we mix
//!   them. LiteCord's own sound (your friends' voices) is left out, so they don't hear themselves.
//!   You can also pick one app. Apps that start playing later are picked up within a second or two.
//!   Without `pw-record` or `pactl`, all of the computer's sound is used (`parec`, the monitor
//!   of the default output), which does include the call.
//! - Windows: process loopback, everything except LiteCord (Windows 10 2004 and newer), else all
//!   of the computer's sound.
//!
//! All of it goes through one 10 ms clock into a stereo 48 kHz `NativeAudioSource`.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use libwebrtc::audio_frame::AudioFrame;
use libwebrtc::audio_source::native::NativeAudioSource;
use libwebrtc::audio_source::AudioSourceOptions;

use super::audio::RATE;

const CHUNK: usize = (RATE / 100) as usize; // 10 ms, per channel
const MAX_QUEUE: usize = RATE as usize * 2 / 5; // 200 ms of stereo
const PREBUFFER: usize = CHUNK * 2 * 3; // 30 ms of stereo before an app is mixed in

/// "No sound", "All apps", or one app ("app:<name>").
pub const NONE: &str = "none";
pub const ALL: &str = "all";

/// The stereo source the stream sound goes into.
pub fn source() -> NativeAudioSource {
    NativeAudioSource::new(AudioSourceOptions { echo_cancellation: false, noise_suppression: false, auto_gain_control: false }, RATE, 2, 100)
}

/// The choices for Settings and the Go Live box: (value, label).
pub fn choices() -> Vec<(String, String)> {
    let mut v = vec![(NONE.to_string(), "No sound".to_string())];
    #[cfg(target_os = "linux")]
    {
        v.push((ALL.to_string(), "All apps (not LiteCord)".to_string()));
        for a in linux::apps() {
            v.push((format!("app:{a}"), a));
        }
    }
    #[cfg(windows)]
    v.push((ALL.to_string(), "All apps (not LiteCord)".to_string()));
    v
}

/// Interleaved stereo samples from one app, waiting to be mixed.
#[derive(Default)]
struct Queue {
    data: VecDeque<f32>,
    started: bool,
}
type Inputs = Arc<Mutex<HashMap<String, Queue>>>;

fn push(inputs: &Inputs, key: &str, samples: &[f32]) {
    let Ok(mut m) = inputs.lock() else { return };
    let q = m.entry(key.to_string()).or_default();
    q.data.extend(samples.iter().copied());
    while q.data.len() > MAX_QUEUE {
        q.data.pop_front();
    }
}

/// Stops capturing when dropped.
pub struct ShareAudio {
    stop: Arc<AtomicBool>,
}

impl Drop for ShareAudio {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

/// Starts capturing `choice` into `src`. `None` for "No sound" or when nothing can be captured
/// here (the reason is in the error).
pub fn start(choice: &str, src: NativeAudioSource, rt: tokio::runtime::Handle) -> Result<Option<ShareAudio>, String> {
    if choice == NONE || choice.is_empty() {
        return Ok(None);
    }
    let stop = Arc::new(AtomicBool::new(false));
    let inputs: Inputs = Arc::default();
    capture(choice, inputs.clone(), stop.clone())?;
    // The mixer: one 10 ms stereo frame at a time, to WebRTC on the engine's runtime.
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<Vec<i16>>();
    rt.spawn(async move {
        while let Some(c) = rx.recv().await {
            let frame = AudioFrame { data: c.into(), sample_rate: RATE, num_channels: 2, samples_per_channel: CHUNK as u32 };
            let _ = src.capture_frame(&frame).await;
        }
    });
    let st = stop.clone();
    std::thread::Builder::new()
        .name("stream-sound-mix".into())
        .spawn(move || {
            let mut next = Instant::now();
            let mut mix = vec![0f32; CHUNK * 2];
            while !st.load(Ordering::SeqCst) {
                next += Duration::from_millis(10);
                let now = Instant::now();
                if next > now {
                    std::thread::sleep(next - now);
                } else if now - next > Duration::from_millis(200) {
                    next = now; // we fell far behind (the computer slept): don't rush to catch up
                }
                mix.iter_mut().for_each(|m| *m = 0.0);
                if let Ok(mut m) = inputs.lock() {
                    for q in m.values_mut() {
                        if !q.started {
                            if q.data.len() < PREBUFFER {
                                continue;
                            }
                            q.started = true;
                        }
                        if q.data.len() < CHUNK * 2 {
                            q.started = false; // ran dry: let a little build up again
                        }
                        for s in mix.iter_mut() {
                            match q.data.pop_front() {
                                Some(v) => *s += v,
                                None => break,
                            }
                        }
                    }
                }
                let c: Vec<i16> = mix.iter().map(|f| (f.clamp(-1.0, 1.0) * 32767.0) as i16).collect();
                if tx.send(c).is_err() {
                    break;
                }
            }
        })
        .map_err(|e| e.to_string())?;
    Ok(Some(ShareAudio { stop }))
}

#[cfg(target_os = "linux")]
fn capture(choice: &str, inputs: Inputs, stop: Arc<AtomicBool>) -> Result<(), String> {
    linux::capture(choice, inputs, stop)
}

#[cfg(windows)]
fn capture(_choice: &str, inputs: Inputs, stop: Arc<AtomicBool>) -> Result<(), String> {
    windows::capture(inputs, stop)
}

#[cfg(not(any(target_os = "linux", windows)))]
fn capture(_choice: &str, _inputs: Inputs, _stop: Arc<AtomicBool>) -> Result<(), String> {
    Err("stream sound isn't available on this system yet".into())
}

/// Reads interleaved f32 stereo from a child's stdout into the mix, until it ends.
#[cfg(target_os = "linux")]
fn read_child(mut out: impl std::io::Read, inputs: Inputs, key: String) {
    let mut buf = vec![0u8; CHUNK * 2 * 4];
    let mut have = 0usize;
    let mut f = Vec::with_capacity(CHUNK * 2);
    loop {
        match out.read(&mut buf[have..]) {
            Ok(0) | Err(_) => break,
            Ok(n) => have += n,
        }
        let whole = have / 4 * 4;
        f.clear();
        f.extend(buf[..whole].chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])));
        buf.copy_within(whole..have, 0);
        have -= whole;
        push(&inputs, &key, &f);
    }
    if let Ok(mut m) = inputs.lock() {
        m.remove(&key);
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use std::process::{Child, Command, Stdio};

    /// One app that's playing sound (a PipeWire output stream).
    pub(super) struct Node {
        pub serial: String,
        pub app: String,
    }

    fn our_names() -> (String, String) {
        let exe = std::env::current_exe().ok().and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned())).unwrap_or_else(|| "litecord".into());
        (std::process::id().to_string(), exe)
    }

    /// Is this stream LiteCord's own sound (calls, sound effects)?
    pub(super) fn is_ours(props: &serde_json::Map<String, serde_json::Value>, pid: &str, exe: &str) -> bool {
        let s = |k: &str| props.get(k).and_then(|v| v.as_str()).unwrap_or("");
        if s("application.process.id") == pid {
            return true;
        }
        // ALSA apps (that's us, through cpal) are named "PipeWire ALSA [<program>]", where the
        // program name may be cut to 15 characters.
        let alsa = s("application.name").strip_prefix("PipeWire ALSA [").and_then(|r| r.strip_suffix(']')).unwrap_or("");
        let node = s("node.name").strip_prefix("alsa_playback.").unwrap_or("");
        let same = |n: &str| !n.is_empty() && (exe.starts_with(n) || n.starts_with(exe));
        same(alsa) || same(node)
    }

    /// A readable name for an app.
    pub(super) fn app_name(props: &serde_json::Map<String, serde_json::Value>) -> Option<String> {
        let s = |k: &str| props.get(k).and_then(|v| v.as_str()).map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
        let bin = s("application.process.binary").map(|b| b.trim_end_matches(" (deleted)").to_string());
        let name = s("application.name")?;
        let name = name.strip_prefix("PipeWire ALSA [").and_then(|r| r.strip_suffix(']')).map(str::to_string).unwrap_or(name);
        // Discord's voice calls name themselves "WEBRTC VoiceEngine": the program name says more.
        Some(match bin {
            Some(b) if name.eq_ignore_ascii_case("WEBRTC VoiceEngine") || name.eq_ignore_ascii_case("playStream") => b,
            _ => name,
        })
    }

    /// Apps playing sound right now (not LiteCord). Loopbacks and virtual devices you set up
    /// yourself (no app name) are left out, since they'd carry your microphone.
    pub(super) fn nodes() -> Option<Vec<Node>> {
        let out = Command::new("pactl").args(["-f", "json", "list", "sink-inputs"]).stderr(Stdio::null()).output().ok()?;
        if !out.status.success() {
            return None;
        }
        let list: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
        let (pid, exe) = our_names();
        Some(parse_nodes(&list, &pid, &exe))
    }

    pub(super) fn parse_nodes(list: &serde_json::Value, pid: &str, exe: &str) -> Vec<Node> {
        let mut v = Vec::new();
        for s in list.as_array().into_iter().flatten() {
            let Some(props) = s.get("properties").and_then(|p| p.as_object()) else { continue };
            if is_ours(props, pid, exe) {
                continue;
            }
            let serial = props.get("object.serial").and_then(|v| v.as_str()).map(str::to_string).or_else(|| s.get("index").map(|i| i.to_string()));
            let (Some(serial), Some(app)) = (serial, app_name(props)) else { continue };
            v.push(Node { serial, app });
        }
        v
    }

    pub(super) fn apps() -> Vec<String> {
        let mut names: Vec<String> = nodes().unwrap_or_default().into_iter().map(|n| n.app).collect();
        names.sort_by_key(|n| n.to_lowercase());
        names.dedup();
        names
    }

    fn record(target: &str) -> std::io::Result<Child> {
        Command::new("pw-record")
            .args(["--raw", "--target", target, "--format", "f32", "--rate", "48000", "--channels", "2", "--latency", "20ms"])
            .args(["-P", "{ node.name = litecord-stream-sound, media.role = Communication }"])
            .arg("-")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
    }

    fn whole_computer() -> std::io::Result<Child> {
        Command::new("parec")
            .args(["-d", "@DEFAULT_MONITOR@", "--format=float32le", "--rate=48000", "--channels=2", "--raw", "--latency-msec=20"])
            .args(["--client-name=LiteCord stream sound"])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
    }

    fn feed(mut child: Child, inputs: &Inputs, key: &str) -> Child {
        if let Some(out) = child.stdout.take() {
            let (i, k) = (inputs.clone(), key.to_string());
            let _ = std::thread::Builder::new().name("stream-sound-read".into()).spawn(move || read_child(out, i, k));
        }
        child
    }

    pub(super) fn capture(choice: &str, inputs: Inputs, stop: Arc<AtomicBool>) -> Result<(), String> {
        let per_app = nodes().is_some() && Command::new("pw-record").arg("--version").stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok();
        if !per_app {
            if choice != ALL {
                return Err("picking one app needs PipeWire (pw-record and pactl)".into());
            }
            let child = whole_computer().map_err(|e| format!("couldn't capture your computer's sound ({e})"))?;
            let mut child = feed(child, &inputs, "monitor");
            std::thread::spawn(move || {
                while !stop.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_millis(200));
                }
                let _ = child.kill();
                let _ = child.wait();
            });
            return Ok(());
        }
        let want_app = choice.strip_prefix("app:").map(str::to_string);
        std::thread::Builder::new()
            .name("stream-sound-apps".into())
            .spawn(move || {
                let mut running: HashMap<String, Child> = HashMap::new();
                let mut last_scan = Instant::now() - Duration::from_secs(10);
                while !stop.load(Ordering::SeqCst) {
                    if last_scan.elapsed() >= Duration::from_millis(1500) {
                        last_scan = Instant::now();
                        let now: Vec<Node> = nodes().unwrap_or_default().into_iter().filter(|n| want_app.as_ref().map(|a| *a == n.app).unwrap_or(true)).collect();
                        // Stop recording apps that went away.
                        running.retain(|serial, child| {
                            let keep = now.iter().any(|n| n.serial == *serial) && matches!(child.try_wait(), Ok(None));
                            if !keep {
                                let _ = child.kill();
                                let _ = child.wait();
                                if let Ok(mut m) = inputs.lock() {
                                    m.remove(serial);
                                }
                            }
                            keep
                        });
                        for n in now {
                            if running.contains_key(&n.serial) {
                                continue;
                            }
                            if let Ok(c) = record(&n.serial) {
                                running.insert(n.serial.clone(), feed(c, &inputs, &n.serial));
                            }
                        }
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
                for (_, mut c) in running {
                    let _ = c.kill();
                    let _ = c.wait();
                }
            })
            .map_err(|e| e.to_string())?;
        Ok(())
    }
}

#[cfg(windows)]
mod windows {
    use super::*;

    pub(super) fn capture(inputs: Inputs, stop: Arc<AtomicBool>) -> Result<(), String> {
        let (ok_tx, ok_rx) = std::sync::mpsc::sync_channel::<Result<(), String>>(1);
        std::thread::Builder::new()
            .name("stream-sound".into())
            .spawn(move || {
                use wasapi::*;
                let _ = initialize_mta().ok();
                let fmt = WaveFormat::new(32, 32, SampleType::Float, RATE as usize, 2, None);
                let mode = StreamMode::EventsShared { autoconvert: true, buffer_duration_hns: 200_000 };
                // Everything except LiteCord and anything it started.
                let client = AudioClient::new_application_loopback_client(std::process::id(), false).and_then(|mut c| {
                    c.initialize_client(&fmt, Direction::Capture, mode, None)?;
                    Ok(c)
                });
                let client = match client {
                    Ok(c) => c,
                    Err(e) => {
                        let _ = ok_tx.send(Err(format!("couldn't capture your computer's sound ({e}); it needs Windows 10 version 2004 or newer")));
                        return;
                    }
                };
                let (Ok(event), Ok(cap)) = (client.set_get_eventhandle(), client.get_audiocaptureclient()) else {
                    let _ = ok_tx.send(Err("couldn't capture your computer's sound".into()));
                    return;
                };
                if let Err(e) = client.start_stream() {
                    let _ = ok_tx.send(Err(format!("couldn't capture your computer's sound ({e})")));
                    return;
                }
                let _ = ok_tx.send(Ok(()));
                let mut bytes: VecDeque<u8> = VecDeque::new();
                let mut f: Vec<f32> = Vec::new();
                while !stop.load(Ordering::SeqCst) {
                    // Nothing comes while nothing is playing, so don't wait long.
                    let _ = event.wait_for_event(100);
                    let mut failed = false;
                    while let Ok(Some(n)) = cap.get_next_packet_size() {
                        if n == 0 {
                            break;
                        }
                        if cap.read_from_device_to_deque(&mut bytes).is_err() {
                            failed = true;
                            break;
                        }
                    }
                    if failed {
                        break;
                    }
                    let whole = bytes.len() / 4 * 4;
                    f.clear();
                    let raw: Vec<u8> = bytes.drain(..whole).collect();
                    f.extend(raw.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])));
                    if !f.is_empty() {
                        push(&inputs, "apps", &f);
                    }
                }
                let _ = client.stop_stream();
            })
            .map_err(|e| e.to_string())?;
        ok_rx.recv_timeout(Duration::from_secs(5)).map_err(|_| "Windows didn't answer".to_string())?
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::linux::*;

    #[test]
    fn leaves_out_litecord_and_loopbacks() {
        let list = serde_json::json!([
            {"index": 59, "properties": {"node.name": "cmm_mic_playback", "media.name": "Mic into Call Mix", "object.serial": "59"}},
            {"index": 1385, "properties": {"application.name": "WEBRTC VoiceEngine", "application.process.id": "34577", "application.process.binary": "Discord", "object.serial": "1385"}},
            {"index": 2390, "properties": {"application.name": "PipeWire ALSA [litecord]", "node.name": "alsa_playback.litecord", "object.serial": "2390"}},
            {"index": 2391, "properties": {"application.name": "PipeWire ALSA [LiteCord-2.3.0-l]", "node.name": "alsa_playback.LiteCord-2.3.0-l", "object.serial": "2391"}},
            {"index": 2936, "properties": {"application.name": "Brave", "application.process.id": "5186", "object.serial": "2936"}},
            {"index": 3000, "properties": {"application.name": "Mine", "application.process.id": "777", "object.serial": "3000"}},
            {"index": 3001, "properties": {"application.name": "PipeWire ALSA [Game.exe]", "node.name": "alsa_playback.Game.exe", "object.serial": "3001"}}
        ]);
        let a = parse_nodes(&list, "777", "litecord");
        let names: Vec<_> = a.iter().map(|n| n.app.as_str()).collect();
        assert_eq!(names, ["Discord", "LiteCord-2.3.0-l", "Brave", "Game.exe"]);
        let b = parse_nodes(&list, "777", "LiteCord-2.3.0-linux-x86_64");
        let names: Vec<_> = b.iter().map(|n| n.app.as_str()).collect();
        assert_eq!(names, ["Discord", "litecord", "Brave", "Game.exe"]);
    }
}

#[cfg(all(test, target_os = "linux"))]
mod live {
    /// Records 2 seconds of every app playing right now and prints how loud each was.
    /// `cargo test share_audio::live -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn records_apps_playing_now() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::sync::Arc;
        println!("choices: {:?}", super::choices());
        let inputs: super::Inputs = Arc::default();
        let stop = Arc::new(AtomicBool::new(false));
        super::capture(super::ALL, inputs.clone(), stop.clone()).unwrap();
        let mut seen = std::collections::HashMap::<String, (usize, f32)>::new();
        for _ in 0..20 {
            std::thread::sleep(std::time::Duration::from_millis(100));
            let mut m = inputs.lock().unwrap();
            for (k, q) in m.iter_mut() {
                let e = seen.entry(k.clone()).or_default();
                e.0 += q.data.len();
                e.1 += q.data.drain(..).map(|s| s * s).sum::<f32>();
            }
        }
        stop.store(true, Ordering::SeqCst);
        for (k, (n, sum)) in &seen {
            println!("node {k}: {} ms, rms {:.4}", n / 96, (sum / (*n).max(1) as f32).sqrt());
        }
        if seen.is_empty() {
            println!("no app was playing sound");
        }
    }
}

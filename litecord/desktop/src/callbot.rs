//! Headless call test: joins a room and its voice channel with the real call engine (no window)
//! and prints one line of JSON a second: who's in the call, connection states, audio bytes
//! and levels, video frames. Typed commands on stdin: `mute`, `unmute`, `deaf`, `undeaf`,
//! `cam`, `nocam`, `screen`, `noscreen`, `leave`, `join`, `reopen`, `chat <text>`, `quit`.
//!
//!     callbot --room dm-<cid> --name Bot [--base URL] [--voice default-lounge] [--secs 60]
//!
//! Point it at a fake microphone and a silent speaker for tests, e.g. with
//! PULSE_SOURCE=<null sink>.monitor PULSE_SINK=<null sink>.

#[allow(dead_code)]
mod call;

use std::io::BufRead;
use std::time::{Duration, Instant};

use call::{CallEvent, Cmd};

/// LC_WEBRTC_LOG=1 prints libwebrtc's own log (it goes through the `log` crate).
struct StderrLog;
impl log::Log for StderrLog {
    fn enabled(&self, _: &log::Metadata) -> bool { true }
    fn log(&self, r: &log::Record) { eprintln!("[{}] {}", r.target(), r.args()); }
    fn flush(&self) {}
}

fn main() {
    if std::env::var_os("LC_WEBRTC_LOG").is_some() {
        let _ = log::set_logger(&StderrLog);
        log::set_max_level(log::LevelFilter::Debug);
    }
    let args: Vec<String> = std::env::args().collect();
    let arg = |k: &str| args.iter().position(|a| a == k).and_then(|i| args.get(i + 1)).cloned();
    let base = arg("--base").unwrap_or_else(|| "https://litecord.litecord.workers.dev".into());
    if args.iter().any(|a| a == "--list-devices") {
        let (i, o) = call::audio::Audio::devices();
        println!("microphones: {i:?}\nspeakers: {o:?}");
        return;
    }
    let room = arg("--room").expect("--room <name> is needed");
    let name = arg("--name").unwrap_or_else(|| "Callbot".into());
    let voice = arg("--voice").unwrap_or_else(|| call::DM_VOICE.into());
    let secs: u64 = arg("--secs").and_then(|s| s.parse().ok()).unwrap_or(60);
    let uid = arg("--uid").unwrap_or_else(|| call::room::rid(8));

    let (etx, erx) = async_channel::unbounded::<CallEvent>();
    let h = call::start(base, etx);
    // --mic / --speaker pick devices by name (e.g. a null sink's "Monitor of …" for a fake mic).
    h.send(Cmd::Devices { mic: arg("--mic").unwrap_or_default(), speaker: arg("--speaker").unwrap_or_default() });
    let open = Cmd::Open { room, uid, name, status: "online".into(), custom: String::new(), saved: None };
    h.send(open.clone());
    if voice != "none" {
        h.send(Cmd::JoinVoice(voice.clone()));
    }

    let (ltx, lrx) = std::sync::mpsc::channel::<String>();
    std::thread::spawn(move || {
        for line in std::io::stdin().lock().lines().map_while(Result::ok) {
            if ltx.send(line).is_err() {
                return;
            }
        }
    });

    let start = Instant::now();
    let mut next = Instant::now();
    while start.elapsed() < Duration::from_secs(secs) {
        while let Ok(ev) = erx.try_recv() {
            match ev {
                CallEvent::Toast(t) => println!("{{\"toast\":{}}}", serde_json::json!(t)),
                CallEvent::Sound(s) => println!("{{\"sound\":\"{s}\"}}"),
                CallEvent::Chat(ch) => {
                    let v = h.view();
                    if let Some(m) = v.history.get(&ch).and_then(|l| l.last()) {
                        println!("{{\"chat\":{},\"from\":{},\"text\":{}}}", serde_json::json!(ch), serde_json::json!(m.name), serde_json::json!(m.text));
                    }
                }
                _ => {}
            }
        }
        while let Ok(line) = lrx.try_recv() {
            let line = line.trim();
            let (cmd, rest) = line.split_once(' ').unwrap_or((line, ""));
            match cmd {
                "mute" => h.send(Cmd::SetMuted(true)),
                "unmute" => h.send(Cmd::SetMuted(false)),
                "deaf" => h.send(Cmd::SetDeaf(true)),
                "undeaf" => h.send(Cmd::SetDeaf(false)),
                "cam" => h.send(Cmd::Camera(true)),
                "nocam" => h.send(Cmd::Camera(false)),
                "screen" => h.send(Cmd::Screen(Some(rest.parse().ok()))),
                "noscreen" => h.send(Cmd::Screen(None)),
                "leave" => h.send(Cmd::LeaveVoice),
                // Leave the room and come back with a new connection, like hanging up and calling again.
                "reopen" => {
                    h.send(Cmd::LeaveVoice);
                    h.send(Cmd::Close);
                    h.send(open.clone());
                    h.send(Cmd::JoinVoice(voice.clone()));
                }
                "join" => h.send(Cmd::JoinVoice(if rest.is_empty() { voice.clone() } else { rest.into() })),
                "chat" => {
                    let v = h.view();
                    let ch = v.channels.iter().find(|c| c.kind == "text").map(|c| c.id.clone()).unwrap_or_default();
                    h.send(Cmd::Chat { ch, text: rest.into() });
                }
                "quit" => {
                    h.send(Cmd::Quit);
                    std::thread::sleep(Duration::from_millis(300));
                    return;
                }
                _ => {}
            }
        }
        if Instant::now() >= next {
            next += Duration::from_secs(1);
            let v = h.view();
            let people: Vec<_> = v.people.iter().map(|p| serde_json::json!({"name": p.name, "voice": p.state.voice, "muted": p.state.muted, "deaf": p.state.deaf,
                "cam": p.state.cam.is_some(), "screen": p.state.screen.is_some(), "speaking": v.speaking.contains(&p.id)})).collect();
            let stats: Vec<_> = v.stats.iter().map(|s| serde_json::json!({"peer": v.people.iter().find(|p| p.id == s.id).map(|p| p.name.clone()).unwrap_or_default(),
                "state": s.state, "in": s.audio_in_bytes, "out": s.audio_out_bytes, "level": (s.audio_in_level * 1000.0).round() / 1000.0,
                "vin": s.video_in_frames, "vout": s.video_out_frames, "codecs": s.codecs})).collect();
            let frames: Vec<_> = h.frames.keys().into_iter().map(|k| {
                let i = h.frames.get(&k);
                serde_json::json!({"key": k, "w": i.as_ref().map(|i| i.width), "h": i.as_ref().map(|i| i.height), "seq": i.map(|i| i.seq)})
            }).collect();
            println!("{}", serde_json::json!({"t": start.elapsed().as_secs(), "connected": v.connected, "ready": v.ready, "host": v.host, "voice": v.voice,
                "people": people, "stats": stats, "frames": frames, "mic_failed": v.mic_failed}));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    h.send(Cmd::Quit);
    std::thread::sleep(Duration::from_millis(300));
}

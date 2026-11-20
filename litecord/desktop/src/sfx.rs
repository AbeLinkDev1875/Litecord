//! Sound effects, made in code like the web app's (`SFX` in index.html), so both sound the same:
//! joining and leaving calls, people joining, mute and deafen, streams, messages, friend
//! requests, and the ringing for incoming and outgoing calls.
//!
//! Each sound is drawn into a buffer at the speakers' own sample rate and played on its own
//! short-lived output stream, on one background thread, so the UI never waits on audio.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::sync::OnceLock;
use std::time::Duration;

use cpal::traits::{DeviceTrait, StreamTrait};
use cpal::Sample;

/// The sounds people can turn off one by one in Settings, with their labels (the web app's
/// `SFX_LABELS`). "mute" also covers unmute, "deafen" undeafen, "streamStart" streamStop.
pub const LABELS: &[(&str, &str)] = &[
    ("message", "New message"),
    ("join", "You join a call"),
    ("leave", "You leave a call"),
    ("userJoin", "Someone joins your call"),
    ("userLeave", "Someone leaves your call"),
    ("mute", "Mute and unmute"),
    ("deafen", "Deafen and undeafen"),
    ("streamStart", "A stream starts or stops"),
    ("ring", "Incoming call"),
    ("calling", "Outgoing call"),
    ("friend", "Friend requests"),
];

/// Which setting a sound belongs to.
pub fn setting_of(name: &str) -> &str {
    match name {
        "unmute" => "mute",
        "undeafen" => "deafen",
        "streamStop" => "streamStart",
        n => n,
    }
}

struct Play {
    name: String,
    speaker: String,
    volume: f32,
}

static PLAYER: OnceLock<mpsc::Sender<Play>> = OnceLock::new();

/// Plays a sound on `speaker` (empty: the system's default) at `volume` (0 to 1).
pub fn play(name: &str, speaker: &str, volume: f32) {
    if volume <= 0.0 {
        return;
    }
    let tx = PLAYER.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Play>();
        let _ = std::thread::Builder::new().name("sfx".into()).spawn(move || player(rx));
        tx
    });
    let _ = tx.send(Play { name: name.into(), speaker: speaker.into(), volume });
}

fn player(rx: mpsc::Receiver<Play>) {
    // Streams still playing, with when they can be closed and whether they broke.
    let mut playing: Vec<(cpal::Stream, std::time::Instant, Arc<AtomicBool>)> = Vec::new();
    loop {
        let next = rx.recv_timeout(Duration::from_millis(50));
        let now = std::time::Instant::now();
        // A broken stream is closed right away (see `audio::on_stream_error`).
        playing.retain(|(_, until, broken)| *until > now && !broken.load(Ordering::SeqCst));
        match next {
            Ok(p) => match open(&p) {
                Ok((s, len, broken)) => playing.push((s, now + len + Duration::from_millis(300), broken)),
                Err(e) => eprintln!("sound effect {}: {e}", p.name),
            },
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
}

fn open(p: &Play) -> Result<(cpal::Stream, Duration, Arc<AtomicBool>), String> {
    let dev = crate::call::audio::find_device(false, &p.speaker).ok_or("no speakers found")?;
    let cfg = dev.default_output_config().map_err(|e| e.to_string())?;
    let channels = cfg.channels() as usize;
    let rate = cfg.sample_rate();
    let mut buf = render(&p.name, rate);
    if buf.is_empty() {
        return Err("unknown sound".into());
    }
    let len = Duration::from_secs_f32(buf.len() as f32 / rate as f32);
    for s in buf.iter_mut() {
        *s = (*s * p.volume).clamp(-1.0, 1.0);
    }
    let mut pos = 0usize;
    let mut fill = move |out: &mut [f32]| {
        for f in out.chunks_mut(channels) {
            let s = buf.get(pos).copied().unwrap_or(0.0);
            pos += 1;
            f.iter_mut().for_each(|c| *c = s);
        }
    };
    let config: cpal::StreamConfig = cfg.config();
    let broken = Arc::new(AtomicBool::new(false));
    let on_err = crate::call::audio::on_stream_error("sound effects", broken.clone());
    macro_rules! build {
        ($t:ty) => {{
            let mut tmp: Vec<f32> = Vec::new();
            dev.build_output_stream::<$t, _, _>(&config, move |d: &mut [$t], _| {
                tmp.resize(d.len(), 0.0);
                fill(&mut tmp);
                for (o, s) in d.iter_mut().zip(&tmp) {
                    *o = <$t>::from_sample(*s);
                }
            }, on_err, None)
        }};
    }
    let s = match cfg.sample_format() {
        cpal::SampleFormat::F32 => dev.build_output_stream::<f32, _, _>(&config, move |d: &mut [f32], _| fill(d), on_err, None),
        cpal::SampleFormat::I16 => build!(i16),
        cpal::SampleFormat::I32 => build!(i32),
        cpal::SampleFormat::U16 => build!(u16),
        cpal::SampleFormat::U8 => build!(u8),
        f => return Err(format!("unsupported sample format {f}")),
    }
    .map_err(|e| e.to_string())?;
    s.play().map_err(|e| e.to_string())?;
    Ok((s, len, broken))
}

#[derive(Clone, Copy)]
enum Wave {
    Sine,
    Triangle,
}

/// One note, like the web app's `tone()`: a quick fade in to `vol`, then an exponential fade
/// out over `dur` seconds, with the pitch gliding from `f` to `f2`.
struct Tone {
    at: f32,
    f: f32,
    f2: f32,
    dur: f32,
    wave: Wave,
    vol: f32,
}

fn t(at: f32, f: f32, dur: f32, wave: Wave, vol: f32) -> Tone {
    Tone { at, f, f2: f, dur, wave, vol }
}

fn notes(name: &str) -> Vec<Tone> {
    use Wave::*;
    let glide = |f: f32, f2: f32, dur: f32, vol: f32| Tone { at: 0., f, f2, dur, wave: Sine, vol };
    match name {
        "ring" => (0..2).flat_map(|i| {
            let at = i as f32 * 0.4;
            [t(at, 784., 0.18, Sine, 0.14), t(at + 0.18, 659., 0.2, Sine, 0.14)]
        }).collect(),
        "calling" => vec![t(0., 440., 0.35, Sine, 0.08), t(0., 480., 0.35, Sine, 0.06)],
        "friend" => [659., 880., 1175.].iter().enumerate().map(|(i, f)| t(i as f32 * 0.07, *f, 0.22, Triangle, 0.12)).collect(),
        "join" => vec![t(0., 587., 0.13, Triangle, 0.2), t(0.1, 880., 0.24, Triangle, 0.2)],
        "leave" => vec![t(0., 880., 0.13, Triangle, 0.2), t(0.1, 587., 0.24, Triangle, 0.2)],
        "userJoin" => vec![t(0., 698., 0.09, Sine, 0.12), t(0.08, 1047., 0.16, Sine, 0.12)],
        "userLeave" => vec![t(0., 1047., 0.09, Sine, 0.12), t(0.08, 698., 0.16, Sine, 0.12)],
        "mute" => vec![glide(560., 300., 0.13, 0.15)],
        "unmute" => vec![glide(300., 560., 0.13, 0.15)],
        "deafen" => [440., 330., 247.].iter().enumerate().map(|(i, f)| t(i as f32 * 0.08, *f, 0.1, Sine, 0.13)).collect(),
        "undeafen" => [247., 330., 440.].iter().enumerate().map(|(i, f)| t(i as f32 * 0.08, *f, 0.1, Sine, 0.13)).collect(),
        "message" => vec![t(0., 988., 0.08, Sine, 0.12), t(0.07, 1319., 0.18, Sine, 0.12)],
        "streamStart" => vec![
            Tone { at: 0., f: 392., f2: 784., dur: 0.25, wave: Triangle, vol: 0.15 },
            t(0.22, 1175., 0.18, Sine, 0.1),
        ],
        "streamStop" => vec![Tone { at: 0., f: 784., f2: 392., dur: 0.3, wave: Triangle, vol: 0.15 }],
        _ => vec![],
    }
}

/// Draws a sound at `rate` samples a second (mono, -1 to 1).
pub fn render(name: &str, rate: u32) -> Vec<f32> {
    let notes = notes(name);
    let rate_f = rate as f32;
    let end = notes.iter().map(|n| n.at + n.dur + 0.02).fold(0.0f32, f32::max);
    let mut out = vec![0.0f32; (end * rate_f).ceil() as usize];
    const ATTACK: f32 = 0.005;
    const FLOOR: f32 = 0.0001;
    for n in &notes {
        let start = (n.at * rate_f) as usize;
        let count = ((n.dur + 0.02) * rate_f) as usize;
        let mut phase = 0.0f32;
        for i in 0..count {
            let tt = i as f32 / rate_f;
            // The pitch glides exponentially, like exponentialRampToValueAtTime.
            let f = if n.f2 != n.f { n.f * (n.f2 / n.f).powf((tt / n.dur).min(1.0)) } else { n.f };
            phase = (phase + f / rate_f).fract();
            let wave = match n.wave {
                Wave::Sine => (phase * std::f32::consts::TAU).sin(),
                Wave::Triangle => 1.0 - 4.0 * (phase - 0.5).abs(),
            };
            let gain = if tt < ATTACK {
                FLOOR * (n.vol / FLOOR).powf(tt / ATTACK)
            } else if tt < n.dur {
                n.vol * (FLOOR / n.vol).powf((tt - ATTACK) / (n.dur - ATTACK))
            } else {
                0.0
            };
            if let Some(o) = out.get_mut(start + i) {
                *o += wave * gain;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_sound_renders_and_stays_in_range() {
        for name in ["ring", "calling", "friend", "join", "leave", "userJoin", "userLeave", "mute", "unmute", "deafen", "undeafen", "message", "streamStart", "streamStop"] {
            let b = render(name, 48_000);
            assert!(b.len() > 4_000, "{name} is too short");
            let peak = b.iter().fold(0.0f32, |m, s| m.max(s.abs()));
            assert!(peak > 0.05 && peak <= 1.0, "{name} peak {peak}");
        }
        assert!(render("nope", 48_000).is_empty());
    }

    #[test]
    fn each_setting_has_a_label() {
        for s in ["unmute", "undeafen", "streamStop", "ring"] {
            assert!(LABELS.iter().any(|(k, _)| *k == setting_of(s)));
        }
    }
}

#[cfg(test)]
mod compare {
    /// Prints the same numbers as rendering the web app's SFX in a browser's OfflineAudioContext
    /// (length to the last sample above 0.001, RMS up to there, peak), to check they match.
    /// `cargo test --release sfx::compare -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn stats_like_the_web() {
        for name in ["ring", "join", "mute", "streamStart"] {
            let b = super::render(name, 48_000);
            let last = b.iter().rposition(|s| s.abs() > 1e-3).unwrap_or(0);
            let sum: f32 = b[..=last].iter().map(|s| s * s).sum();
            let peak = b.iter().fold(0f32, |m, s| m.max(s.abs()));
            println!("{name}: len {last} rms {:.4} peak {peak:.3}", (sum / (last + 1) as f32).sqrt());
        }
    }
}

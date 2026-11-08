//! Microphone and speakers, through `cpal` (ALSA/PipeWire/PulseAudio on Linux, WASAPI on
//! Windows), with WebRTC's own audio processing (echo cancellation, noise suppression and
//! automatic gain) on the microphone, like a browser does.
//!
//! The microphone is cleaned up 10 ms at a time and handed to WebRTC as a `NativeAudioSource`.
//! Everyone we hear arrives as a `NativeAudioStream` per person; each gets a small queue that
//! the speaker callback mixes (with that person's volume: 0 when deafened or muted for us).
//! What we play is also fed to the echo canceller, so it can take it out of the microphone.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{mpsc, Arc, Mutex};

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::Sample;
use libwebrtc::audio_frame::AudioFrame;
use libwebrtc::audio_source::native::NativeAudioSource;
use libwebrtc::audio_source::AudioSourceOptions;
use libwebrtc::native::apm::AudioProcessingModule;

pub const RATE: u32 = 48_000;
const CHUNK: usize = (RATE / 100) as usize; // 10 ms
const MAX_QUEUE: usize = RATE as usize * 3 / 10; // keep at most 300 ms per person
const PREBUFFER: usize = CHUNK * 4; // start someone's voice after 40 ms are queued

fn f32_bits(v: f32) -> u32 {
    v.to_bits()
}
fn bits_f32(v: u32) -> f32 {
    f32::from_bits(v)
}

struct Remote {
    queue: VecDeque<f32>,
    started: bool,
    gain: f32,
    level: f32,
    frames: u64,
}

pub struct Shared {
    apm: Mutex<AudioProcessingModule>,
    remotes: Mutex<HashMap<String, Remote>>,
    mic_level: AtomicU32,
    /// Send silence instead of the microphone (muted or deafened).
    pub mic_off: AtomicBool,
    /// The speakers play nothing (deafened).
    pub deaf: AtomicBool,
}

enum Ctl {
    Start { mic: String, speaker: String, source: Option<NativeAudioSource>, rt: tokio::runtime::Handle },
    Stop,
}

/// The audio thread (cpal streams can't move between threads on every platform).
pub struct Audio {
    pub shared: Arc<Shared>,
    ctl: mpsc::Sender<Ctl>,
    pub errors: Arc<Mutex<Vec<String>>>,
}

impl Audio {
    pub fn new() -> Audio {
        let shared = Arc::new(Shared {
            apm: Mutex::new(AudioProcessingModule::new(true, true, true, true)),
            remotes: Mutex::new(HashMap::new()),
            mic_level: AtomicU32::new(0),
            mic_off: AtomicBool::new(false),
            deaf: AtomicBool::new(false),
        });
        let (tx, rx) = mpsc::channel::<Ctl>();
        let errors = Arc::new(Mutex::new(Vec::new()));
        let (sh, errs) = (shared.clone(), errors.clone());
        std::thread::Builder::new()
            .name("litecord-audio".into())
            .spawn(move || audio_thread(rx, sh, errs))
            .expect("audio thread");
        Audio { shared, ctl: tx, errors }
    }

    /// Opens the microphone (feeding `source`, if any) and the speakers.
    pub fn start(&self, mic: &str, speaker: &str, source: Option<NativeAudioSource>) {
        let _ = self.ctl.send(Ctl::Start { mic: mic.into(), speaker: speaker.into(), source, rt: tokio::runtime::Handle::current() });
    }

    /// Closes the microphone and speakers (we left the call).
    pub fn stop(&self) {
        let _ = self.ctl.send(Ctl::Stop);
        if let Ok(mut r) = self.shared.remotes.lock() {
            r.clear();
        }
        self.shared.mic_level.store(0, Ordering::Relaxed);
    }

    pub fn mic_level(&self) -> f32 {
        bits_f32(self.shared.mic_level.load(Ordering::Relaxed))
    }

    pub fn levels(&self) -> HashMap<String, f32> {
        self.shared.remotes.lock().map(|r| r.iter().map(|(k, v)| (k.clone(), v.level)).collect()).unwrap_or_default()
    }

    pub fn frame_counts(&self) -> Vec<u64> {
        self.shared.remotes.lock().map(|r| r.values().map(|v| v.frames).collect()).unwrap_or_default()
    }

    pub fn set_gain(&self, key: &str, gain: f32) {
        if let Ok(mut r) = self.shared.remotes.lock() {
            if let Some(q) = r.get_mut(key) {
                q.gain = gain;
            }
        }
    }

    /// Someone's audio track started: their frames go into a queue under `key`.
    pub fn add_remote(&self, key: &str, gain: f32) {
        if let Ok(mut r) = self.shared.remotes.lock() {
            r.insert(key.to_string(), Remote { queue: VecDeque::new(), started: false, gain, level: 0., frames: 0 });
        }
    }

    pub fn remove_remote(&self, key: &str) {
        if let Ok(mut r) = self.shared.remotes.lock() {
            r.remove(key);
        }
    }

    /// A 10 ms frame from someone (48 kHz mono).
    pub fn push(shared: &Shared, key: &str, data: &[i16]) {
        let Ok(mut r) = shared.remotes.lock() else { return };
        let Some(q) = r.get_mut(key) else { return };
        q.frames += 1;
        let mut sum = 0f32;
        for s in data {
            let f = *s as f32 / 32768.0;
            sum += f * f;
            q.queue.push_back(f);
        }
        let rms = (sum / data.len().max(1) as f32).sqrt();
        q.level = q.level * 0.6 + rms * 0.4;
        while q.queue.len() > MAX_QUEUE {
            q.queue.pop_front();
        }
    }

    /// Microphones and speakers by name, for Settings (the first is the system's default).
    pub fn devices() -> (Vec<String>, Vec<String>) {
        let host = cpal::default_host();
        let ins = host.input_devices().map(|d| d.filter_map(|d| d.name().ok()).collect()).unwrap_or_default();
        let outs = host.output_devices().map(|d| d.filter_map(|d| d.name().ok()).collect()).unwrap_or_default();
        (ins, outs)
    }
}

fn find_device(input: bool, name: &str) -> Option<cpal::Device> {
    let host = cpal::default_host();
    if !name.is_empty() {
        let list = if input { host.input_devices().ok() } else { host.output_devices().ok() };
        for d in list.into_iter().flatten() {
            if d.name().map(|n| n == name).unwrap_or(false) {
                return Some(d);
            }
        }
    }
    if input { host.default_input_device() } else { host.default_output_device() }
}

fn audio_thread(rx: mpsc::Receiver<Ctl>, shared: Arc<Shared>, errors: Arc<Mutex<Vec<String>>>) {
    let mut streams: Vec<cpal::Stream> = Vec::new();
    let err = |e: String| {
        if let Ok(mut v) = errors.lock() {
            v.push(e);
        }
    };
    while let Ok(c) = rx.recv() {
        match c {
            Ctl::Stop => streams.clear(),
            Ctl::Start { mic, speaker, source, rt } => {
                streams.clear();
                match open_output(&speaker, shared.clone()) {
                    Ok(s) => streams.push(s),
                    Err(e) => err(format!("Couldn't open your speakers: {e}")),
                }
                if let Some(src) = source {
                    match open_input(&mic, shared.clone(), src, rt) {
                        Ok(s) => streams.push(s),
                        Err(e) => err(format!("Couldn't use your microphone: {e}. You can still listen.")),
                    }
                }
                for s in &streams {
                    let _ = s.play();
                }
            }
        }
    }
}

/// Simple linear resampler for one channel.
struct Resampler {
    ratio: f64, // input rate / output rate
    pos: f64,
    last: f32,
}

impl Resampler {
    fn new(from: u32, to: u32) -> Self {
        Resampler { ratio: from as f64 / to as f64, pos: 0.0, last: 0.0 }
    }
    fn run(&mut self, input: &[f32], out: &mut Vec<f32>) {
        if (self.ratio - 1.0).abs() < 1e-9 {
            out.extend_from_slice(input);
            return;
        }
        // pos is the position of the next output sample, in input samples, where -1 is `last`.
        while self.pos < input.len() as f64 - 1.0 + 1.0 {
            let i = self.pos.floor() as isize;
            let frac = (self.pos - i as f64) as f32;
            let a = if i < 0 { self.last } else { input[i as usize] };
            let b = if ((i + 1) as usize) < input.len() { input[(i + 1) as usize] } else { a };
            out.push(a + (b - a) * frac);
            self.pos += self.ratio;
        }
        self.pos -= input.len() as f64;
        if let Some(l) = input.last() {
            self.last = *l;
        }
    }
}

fn open_input(name: &str, shared: Arc<Shared>, source: NativeAudioSource, rt: tokio::runtime::Handle) -> Result<cpal::Stream, String> {
    let dev = find_device(true, name).ok_or("no microphone found")?;
    let cfg = dev.default_input_config().map_err(|e| e.to_string())?;
    let channels = cfg.channels() as usize;
    let rate = cfg.sample_rate();
    // Hand the cleaned-up 10 ms chunks to WebRTC on the engine's runtime.
    let (tx, mut chunks) = tokio::sync::mpsc::unbounded_channel::<Vec<i16>>();
    rt.spawn(async move {
        while let Some(c) = chunks.recv().await {
            let frame = AudioFrame { data: c.into(), sample_rate: RATE, num_channels: 1, samples_per_channel: CHUNK as u32 };
            let _ = source.capture_frame(&frame).await;
        }
    });
    let mut rs = Resampler::new(rate, RATE);
    let mut mono: Vec<f32> = Vec::new();
    let mut pending: Vec<f32> = Vec::new();
    let mut handle = move |data: &[f32]| {
        mono.clear();
        for f in data.chunks(channels) {
            mono.push(f.iter().sum::<f32>() / channels as f32);
        }
        rs.run(&mono, &mut pending);
        while pending.len() >= CHUNK {
            let mut c: Vec<i16> = pending.drain(..CHUNK).map(|f| (f.clamp(-1.0, 1.0) * 32767.0) as i16).collect();
            if let Ok(mut apm) = shared.apm.lock() {
                let _ = apm.process_stream(&mut c, RATE as i32, 1);
            }
            let off = shared.mic_off.load(Ordering::Relaxed);
            let mut sum = 0f32;
            for s in &c {
                let f = *s as f32 / 32768.0;
                sum += f * f;
            }
            let rms = if off { 0. } else { (sum / CHUNK as f32).sqrt() };
            let old = bits_f32(shared.mic_level.load(Ordering::Relaxed));
            shared.mic_level.store(f32_bits(old * 0.6 + rms * 0.4), Ordering::Relaxed);
            if off {
                c.iter_mut().for_each(|s| *s = 0);
            }
            let _ = tx.send(c);
        }
    };
    let config: cpal::StreamConfig = cfg.config();
    let on_err = |e: cpal::StreamError| eprintln!("microphone: {e}");
    macro_rules! build {
        ($t:ty) => {
            dev.build_input_stream::<$t, _, _>(&config, move |d: &[$t], _| {
                let f: Vec<f32> = d.iter().map(|s| f32::from_sample(*s)).collect();
                handle(&f)
            }, on_err, None)
        };
    }
    let s = match cfg.sample_format() {
        cpal::SampleFormat::F32 => dev.build_input_stream::<f32, _, _>(&config, move |d: &[f32], _| handle(d), on_err, None),
        cpal::SampleFormat::I16 => build!(i16),
        cpal::SampleFormat::I32 => build!(i32),
        cpal::SampleFormat::U16 => build!(u16),
        cpal::SampleFormat::U8 => build!(u8),
        f => return Err(format!("unsupported sample format {f}")),
    };
    s.map_err(|e| e.to_string())
}

fn open_output(name: &str, shared: Arc<Shared>) -> Result<cpal::Stream, String> {
    let dev = find_device(false, name).ok_or("no speakers found")?;
    let cfg = dev.default_output_config().map_err(|e| e.to_string())?;
    let channels = cfg.channels() as usize;
    let rate = cfg.sample_rate();
    let mut rs = Resampler::new(RATE, rate);
    let mut mix48: Vec<f32> = Vec::new();
    let mut out_buf: VecDeque<f32> = VecDeque::new();
    let mut reverse: Vec<f32> = Vec::new();
    // Fills `out` (device rate, `channels` interleaved) from everyone's queues.
    let mut fill = move |out: &mut [f32]| {
        let frames = out.len() / channels;
        while out_buf.len() < frames {
            // Mix 10 ms at 48 kHz.
            mix48.clear();
            mix48.resize(CHUNK, 0.0);
            let deaf = shared.deaf.load(Ordering::Relaxed);
            if let Ok(mut r) = shared.remotes.lock() {
                for q in r.values_mut() {
                    if !q.started {
                        if q.queue.len() < PREBUFFER {
                            continue;
                        }
                        q.started = true;
                    }
                    if q.queue.len() < CHUNK {
                        q.started = false; // ran dry: wait for a little to build up again
                    }
                    let g = if deaf { 0.0 } else { q.gain };
                    for m in mix48.iter_mut() {
                        match q.queue.pop_front() {
                            Some(s) => *m += s * g,
                            None => break,
                        }
                    }
                }
            }
            reverse.extend_from_slice(&mix48);
            if reverse.len() >= CHUNK {
                let mut c: Vec<i16> = reverse.drain(..CHUNK).map(|f| (f.clamp(-1.0, 1.0) * 32767.0) as i16).collect();
                if let Ok(mut apm) = shared.apm.lock() {
                    let _ = apm.process_reverse_stream(&mut c, RATE as i32, 1);
                }
            }
            let mut dev_rate = Vec::with_capacity(CHUNK * 2);
            rs.run(&mix48, &mut dev_rate);
            out_buf.extend(dev_rate.into_iter().map(|f| f.clamp(-1.0, 1.0)));
        }
        for f in out.chunks_mut(channels) {
            let s = out_buf.pop_front().unwrap_or(0.0);
            for c in f.iter_mut() {
                *c = s;
            }
        }
    };
    let config: cpal::StreamConfig = cfg.config();
    let on_err = |e: cpal::StreamError| eprintln!("speakers: {e}");
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
    };
    s.map_err(|e| e.to_string())
}

/// Makes the WebRTC source our microphone chunks go into.
pub fn mic_source() -> NativeAudioSource {
    // Our own audio processing already ran, so WebRTC's source doesn't need to.
    NativeAudioSource::new(AudioSourceOptions { echo_cancellation: false, noise_suppression: false, auto_gain_control: false }, RATE, 1, 100)
}


#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resampler_keeps_length() {
        let mut r = Resampler::new(44_100, 48_000);
        let mut out = Vec::new();
        for _ in 0..100 {
            r.run(&vec![0.5; 441], &mut out);
        }
        // 100 × 10 ms in, about 100 × 480 out.
        assert!((out.len() as i64 - 48_000).abs() <= 2, "{}", out.len());
        assert!(out.iter().all(|s| (*s - 0.5).abs() < 1e-6 || *s == 0.0));
    }
}

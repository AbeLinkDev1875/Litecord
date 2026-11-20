//! Microphone and speakers, through `cpal` (ALSA/PipeWire/PulseAudio on Linux, WASAPI on
//! Windows), with WebRTC's own audio processing (echo cancellation, noise suppression and
//! automatic gain) on the microphone, like a browser does.
//!
//! The microphone is cleaned up 10 ms at a time and handed to WebRTC as a `NativeAudioSource`.
//! Everyone we hear arrives as a `NativeAudioStream` per person; each gets a small queue that
//! the speaker callback mixes (with that person's volume: 0 when deafened or muted for us).
//! What we play is also fed to the echo canceller, so it can take it out of the microphone.
//!
//! The microphone path, every 10 ms, like the web app's: input volume, AI noise suppression
//! (RNNoise) if picked, WebRTC's processing (echo cancellation, gain control, standard noise
//! suppression), then the voice gate (voice activity or push to talk). A Mic Test plays the
//! result back to you.

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

/// The voice gate's settings (see `VoiceOpts::effective`).
#[derive(Clone)]
struct GateCfg {
    ptt: bool,
    auto: bool,
    threshold: f32,
    ptt_delay_ms: u32,
}

/// Lets the voice through when it's loud enough (or while push-to-talk is held), with a short
/// hold so word endings aren't cut. A port of the web app's voice-gate.js, per 10 ms.
struct Gate {
    floor: f32,
    hold: usize,
    gain: f32,
    level: f32,
    ptt_at: Option<std::time::Instant>,
}

impl Gate {
    fn new() -> Self {
        Gate { floor: -70.0, hold: 0, gain: 0.0, level: -100.0, ptt_at: None }
    }
    /// Gates `c` in place. Returns (level dB, threshold dB, open).
    fn run(&mut self, c: &mut [i16], cfg: &GateCfg, ptt_down: bool) -> (f32, f32, bool) {
        let sum: f32 = c.iter().map(|s| (*s as f32 / 32768.0).powi(2)).sum();
        let db = 10.0 * (sum / c.len().max(1) as f32 + 1e-12).log10();
        self.level = db.max(self.level - 2.25); // the meter falls back slowly
        // Background noise: follows quiet moments quickly, creeps up about 1 dB a second.
        self.floor = if db < self.floor { self.floor * 0.674 + db * 0.326 } else { self.floor + 0.0094 };
        let threshold = if cfg.auto { (self.floor + 15.0).clamp(-62.0, -28.0) } else { cfg.threshold };
        let open = if cfg.ptt {
            if ptt_down {
                self.ptt_at = Some(std::time::Instant::now());
                true
            } else {
                self.ptt_at.map(|t| t.elapsed().as_millis() < cfg.ptt_delay_ms as u128).unwrap_or(false)
            }
        } else {
            if db > threshold {
                self.hold = RATE as usize * 35 / 100;
            } else {
                self.hold = self.hold.saturating_sub(c.len());
            }
            self.hold > 0
        };
        // Open fast, fade out gently.
        let (target, step) = if open { (1.0, 0.02) } else { (0.0, 0.0006) };
        for s in c.iter_mut() {
            self.gain += (target - self.gain) * step;
            *s = (*s as f32 * self.gain) as i16;
        }
        (self.level, threshold, open)
    }
}

pub struct Shared {
    apm: Mutex<AudioProcessingModule>,
    /// (echo cancellation, gain control, standard noise suppression) the processing was made with.
    apm_flags: Mutex<(bool, bool, bool)>,
    ns_ai: AtomicBool,
    gate: Mutex<GateCfg>,
    /// Push-to-talk key held.
    pub ptt: AtomicBool,
    in_gain: AtomicU32,
    out_gain: AtomicU32,
    /// For Settings' meter: level and threshold in dB, and whether the voice gets through.
    meter_db: AtomicU32,
    meter_thr: AtomicU32,
    meter_open: AtomicBool,
    /// Mic Test: play the microphone back to us.
    test: AtomicBool,
    remotes: Mutex<HashMap<String, Remote>>,
    mic_level: AtomicU32,
    /// Send silence instead of the microphone (muted or deafened).
    pub mic_off: AtomicBool,
    /// The speakers play nothing (deafened).
    pub deaf: AtomicBool,
}

enum Ctl {
    Start { mic: String, speaker: String, source: Option<NativeAudioSource>, rt: Option<tokio::runtime::Handle>, input: bool },
    Stop,
}

/// Where a Mic Test plays you back.
const TEST_KEY: &str = "\u{0}mic-test";

/// The audio thread (cpal streams can't move between threads on every platform).
pub struct Audio {
    pub shared: Arc<Shared>,
    ctl: mpsc::Sender<Ctl>,
    pub errors: Arc<Mutex<Vec<String>>>,
}

impl Audio {
    pub fn new() -> Audio {
        let shared = Arc::new(Shared {
            apm: Mutex::new(AudioProcessingModule::new(true, true, true, false)),
            apm_flags: Mutex::new((true, true, false)),
            ns_ai: AtomicBool::new(true),
            gate: Mutex::new(GateCfg { ptt: false, auto: true, threshold: -50.0, ptt_delay_ms: 20 }),
            ptt: AtomicBool::new(false),
            in_gain: AtomicU32::new(f32_bits(1.0)),
            out_gain: AtomicU32::new(f32_bits(1.0)),
            meter_db: AtomicU32::new(f32_bits(-100.0)),
            meter_thr: AtomicU32::new(f32_bits(-50.0)),
            meter_open: AtomicBool::new(false),
            test: AtomicBool::new(false),
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
        let input = source.is_some();
        let _ = self.ctl.send(Ctl::Start { mic: mic.into(), speaker: speaker.into(), source, rt: tokio::runtime::Handle::try_current().ok(), input });
    }

    /// Mic Test: opens the microphone and speakers and plays you back to yourself.
    pub fn start_test(&self, mic: &str, speaker: &str) {
        self.shared.test.store(true, Ordering::Relaxed);
        self.add_remote(TEST_KEY, 1.0);
        let _ = self.ctl.send(Ctl::Start { mic: mic.into(), speaker: speaker.into(), source: None, rt: None, input: true });
    }

    /// Applies microphone and speaker settings.
    pub fn apply(&self, v: &super::VoiceOpts) {
        let (ns, ec, agc, mode, auto, thr) = v.effective();
        let sh = &self.shared;
        sh.in_gain.store(f32_bits(v.in_vol.clamp(0.0, 2.0)), Ordering::Relaxed);
        sh.out_gain.store(f32_bits(v.out_vol.clamp(0.0, 2.0)), Ordering::Relaxed);
        sh.ns_ai.store(ns == "ai", Ordering::Relaxed);
        if let Ok(mut g) = sh.gate.lock() {
            *g = GateCfg { ptt: mode == "ptt", auto, threshold: thr, ptt_delay_ms: v.ptt_delay_ms };
        }
        let flags = (ec, agc, ns == "standard");
        if let (Ok(mut f), Ok(mut apm)) = (sh.apm_flags.lock(), sh.apm.lock()) {
            if *f != flags {
                *f = flags;
                *apm = AudioProcessingModule::new(ec, agc, ec || agc || ns != "off", ns == "standard");
            }
        }
    }

    /// Push-to-talk key held or let go.
    pub fn set_ptt(&self, down: bool) {
        self.shared.ptt.store(down, Ordering::Relaxed);
    }

    /// Settings' meter: (level dB, threshold dB, voice getting through).
    pub fn meter(&self) -> (f32, f32, bool) {
        let sh = &self.shared;
        (bits_f32(sh.meter_db.load(Ordering::Relaxed)), bits_f32(sh.meter_thr.load(Ordering::Relaxed)), sh.meter_open.load(Ordering::Relaxed))
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

pub(crate) fn find_device(input: bool, name: &str) -> Option<cpal::Device> {
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
            Ctl::Start { mic, speaker, source, rt, input } => {
                streams.clear();
                match open_output(&speaker, shared.clone()) {
                    Ok(s) => streams.push(s),
                    Err(e) => err(format!("Couldn't open your speakers: {e}")),
                }
                if input {
                    let send = source.zip(rt);
                    match open_input(&mic, shared.clone(), send) {
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

fn open_input(name: &str, shared: Arc<Shared>, send: Option<(NativeAudioSource, tokio::runtime::Handle)>) -> Result<cpal::Stream, String> {
    let dev = find_device(true, name).ok_or("no microphone found")?;
    let cfg = dev.default_input_config().map_err(|e| e.to_string())?;
    let channels = cfg.channels() as usize;
    let rate = cfg.sample_rate();
    // Hand the cleaned-up 10 ms chunks to WebRTC on the engine's runtime.
    let tx = send.map(|(source, rt)| {
        let (tx, mut chunks) = tokio::sync::mpsc::unbounded_channel::<Vec<i16>>();
        rt.spawn(async move {
            while let Some(c) = chunks.recv().await {
                let frame = AudioFrame { data: c.into(), sample_rate: RATE, num_channels: 1, samples_per_channel: CHUNK as u32 };
                let _ = source.capture_frame(&frame).await;
            }
        });
        tx
    });
    let mut rs = Resampler::new(rate, RATE);
    let mut mono: Vec<f32> = Vec::new();
    let mut pending: Vec<f32> = Vec::new();
    let mut denoise = nnnoiseless::DenoiseState::new();
    let (mut ai_in, mut ai_out) = (vec![0f32; CHUNK], vec![0f32; CHUNK]);
    let mut gate = Gate::new();
    let mut handle = move |data: &[f32]| {
        mono.clear();
        for f in data.chunks(channels) {
            mono.push(f.iter().sum::<f32>() / channels as f32);
        }
        rs.run(&mono, &mut pending);
        while pending.len() >= CHUNK {
            let gain = bits_f32(shared.in_gain.load(Ordering::Relaxed));
            let mut c: Vec<i16> = if shared.ns_ai.load(Ordering::Relaxed) {
                // RNNoise works on 480 samples at 48 kHz, in 16-bit scale.
                for (d, s) in ai_in.iter_mut().zip(pending.drain(..CHUNK)) {
                    *d = (s * gain).clamp(-1.0, 1.0) * 32767.0;
                }
                denoise.process_frame(&mut ai_out, &ai_in);
                ai_out.iter().map(|f| f.clamp(-32768.0, 32767.0) as i16).collect()
            } else {
                pending.drain(..CHUNK).map(|f| ((f * gain).clamp(-1.0, 1.0) * 32767.0) as i16).collect()
            };
            if let Ok(mut apm) = shared.apm.lock() {
                let _ = apm.process_stream(&mut c, RATE as i32, 1);
            }
            let cfg = shared.gate.lock().map(|g| g.clone()).unwrap_or(GateCfg { ptt: false, auto: true, threshold: -50.0, ptt_delay_ms: 0 });
            let (db, thr, open) = gate.run(&mut c, &cfg, shared.ptt.load(Ordering::Relaxed));
            shared.meter_db.store(f32_bits(db), Ordering::Relaxed);
            shared.meter_thr.store(f32_bits(thr), Ordering::Relaxed);
            shared.meter_open.store(open, Ordering::Relaxed);
            let off = shared.mic_off.load(Ordering::Relaxed) && !shared.test.load(Ordering::Relaxed);
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
            if shared.test.load(Ordering::Relaxed) {
                Audio::push(&shared, TEST_KEY, &c);
            }
            if let Some(tx) = &tx {
                let _ = tx.send(c);
            }
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
            let out_gain = bits_f32(shared.out_gain.load(Ordering::Relaxed));
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
                    let g = if deaf { 0.0 } else { q.gain * out_gain };
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

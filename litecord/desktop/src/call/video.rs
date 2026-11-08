//! Video: the frames the UI draws (everyone's camera and screen, ours included), and our own
//! camera and screen captures.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use futures_util::StreamExt;
use libwebrtc::video_frame::native::VideoFrameBufferExt;
use libwebrtc::video_frame::{BoxVideoFrame, I420Buffer, VideoBuffer};
use libwebrtc::video_source::native::NativeVideoSource;
use libwebrtc::video_stream::native::NativeVideoStream;
use libwebrtc::video_track::RtcVideoTrack;

/// One picture, in BGRA (the byte order GPUI draws).
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
    pub seq: u64,
}

/// The newest picture of each video, by key: "<peer id>:<track id>" for others,
/// "<my id>:cam" and "<my id>:screen" for ours.
#[derive(Clone, Default)]
pub struct Frames {
    inner: Arc<Mutex<HashMap<String, Arc<Image>>>>,
    /// Called (from a video thread) when a new picture arrived, so the UI can redraw.
    pub wake: Arc<Mutex<Option<Box<dyn Fn() + Send + Sync>>>>,
}

impl Frames {
    pub fn get(&self, key: &str) -> Option<Arc<Image>> {
        self.inner.lock().ok()?.get(key).cloned()
    }
    pub fn keys(&self) -> Vec<String> {
        self.inner.lock().map(|m| m.keys().cloned().collect()).unwrap_or_default()
    }
    pub fn put(&self, key: &str, width: u32, height: u32, bgra: Vec<u8>) {
        if let Ok(mut m) = self.inner.lock() {
            let seq = m.get(key).map(|i| i.seq + 1).unwrap_or(1);
            m.insert(key.to_string(), Arc::new(Image { width, height, bgra, seq }));
        }
        if let Ok(w) = self.wake.lock() {
            if let Some(w) = w.as_ref() {
                w();
            }
        }
    }
    pub fn remove(&self, key: &str) {
        if let Ok(mut m) = self.inner.lock() {
            m.remove(key);
        }
    }
}

/// Converts any WebRTC frame to BGRA.
pub fn to_bgra(frame: &BoxVideoFrame) -> Option<(u32, u32, Vec<u8>)> {
    let b = &frame.buffer;
    let owned;
    let i420: &I420Buffer = if let Some(i) = b.as_i420() {
        i
    } else {
        owned = if let Some(n) = b.as_native() { n.to_i420() }
            else if let Some(n) = b.as_nv12() { n.to_i420() }
            else if let Some(n) = b.as_i444() { n.to_i420() }
            else if let Some(n) = b.as_i422() { n.to_i420() }
            else if let Some(n) = b.as_i010() { n.to_i420() }
            else { return None };
        &owned
    };
    let (w, h) = (i420.width(), i420.height());
    if w == 0 || h == 0 {
        return None;
    }
    let (sy, su, sv) = i420.strides();
    let (y, u, v) = i420.data();
    let mut out = vec![0u8; (w * h * 4) as usize];
    libwebrtc::native::yuv_helper::i420_to_argb(y, sy, u, su, v, sv, &mut out, w * 4, w as i32, h as i32);
    Some((w, h, out))
}

/// Starts drawing a remote video track into `frames` under `key`.
pub fn watch_remote(frames: &Frames, key: &str, track: RtcVideoTrack) {
    let frames = frames.clone();
    let key = key.to_string();
    std::thread::Builder::new().name("litecord-video-in".into()).spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread().build().expect("runtime");
        rt.block_on(async move {
            let mut stream = NativeVideoStream::new(track);
            let mut last = std::time::Instant::now() - std::time::Duration::from_secs(1);
            while let Some(f) = stream.next().await {
                // At most 30 pictures a second for the UI.
                if last.elapsed() < std::time::Duration::from_millis(30) {
                    continue;
                }
                last = std::time::Instant::now();
                if let Some((w, h, px)) = to_bgra(&f) {
                    frames.put(&key, w, h, px);
                }
            }
            frames.remove(&key);
        });
    }).ok();
}

pub fn stop_remote(frames: &Frames, key: &str) {
    frames.remove(key);
}

/// A running capture (camera or screen). Dropping or `stop()` ends it.
pub struct Capture {
    stop: Arc<AtomicBool>,
    key: String,
    frames: Frames,
}

impl Capture {
    pub fn stop(self) {}
    fn new(frames: &Frames, key: &str) -> (Capture, Arc<AtomicBool>) {
        let stop = Arc::new(AtomicBool::new(false));
        (Capture { stop: stop.clone(), key: key.to_string(), frames: frames.clone() }, stop)
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.frames.remove(&self.key);
    }
}

pub use super::capture::{camera, screen};

impl Capture {
    pub fn camera(frames: &Frames, key: &str, ended: Box<dyn FnOnce() + Send>) -> Result<(NativeVideoSource, Capture), String> {
        let (cap, stop) = Capture::new(frames, key);
        let track = camera(frames.clone(), key.to_string(), stop, ended)?;
        Ok((track, cap))
    }
    pub fn screen(frames: &Frames, key: &str, source: Option<u64>, ended: Box<dyn FnOnce() + Send>) -> Result<(NativeVideoSource, Capture), String> {
        let (cap, stop) = Capture::new(frames, key);
        let track = screen(frames.clone(), key.to_string(), source, stop, ended)?;
        Ok((track, cap))
    }
}

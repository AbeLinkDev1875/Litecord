//! Camera and screen capture, each on its own thread, feeding a WebRTC `NativeVideoSource`
//! (and our own preview tile through `Frames`).
//!
//! - Camera: `nokhwa` (V4L2 on Linux, Media Foundation on Windows). MJPEG is decoded with the
//!   pure-Rust `image` crate. `LITECORD_FAKE_CAMERA=1` uses a moving test picture instead, for
//!   testing on machines without a camera.
//! - Screen: libwebrtc's desktop capturer. On Linux under Wayland it asks through the
//!   xdg-desktop-portal (PipeWire), so the system shows its own "share your screen" picker;
//!   on X11 and Windows it captures the screen directly.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use libwebrtc::native::yuv_helper;
use libwebrtc::video_frame::{I420Buffer, VideoBuffer, VideoFrame, VideoRotation};
use libwebrtc::video_source::native::NativeVideoSource;
use libwebrtc::video_source::VideoResolution;

use super::video::Frames;

/// Live limits for a capture, changeable while it runs: the tallest picture to send (0 = as
/// captured) and frames a second.
pub struct Limits {
    pub max_h: AtomicU32,
    pub fps: AtomicU32,
}

impl Limits {
    pub fn new(q: &super::Quality) -> Arc<Limits> {
        let l = Arc::new(Limits { max_h: AtomicU32::new(0), fps: AtomicU32::new(30) });
        l.set(q);
        l
    }
    pub fn set(&self, q: &super::Quality) {
        self.max_h.store(q.height, Ordering::Relaxed);
        self.fps.store(q.fps.clamp(5, 60), Ordering::Relaxed);
    }
    fn frame_time(&self) -> Duration {
        Duration::from_micros(1_000_000 / self.fps.load(Ordering::Relaxed).max(1) as u64)
    }
}

/// The size to send a `w`×`h` picture at, no taller than `max_h` (0 = no limit), even numbers.
pub fn fit(w: u32, h: u32, max_h: u32) -> (u32, u32) {
    if max_h == 0 || h <= max_h || h == 0 {
        return (w & !1, h & !1);
    }
    let nh = max_h & !1;
    ((((w as u64 * nh as u64) / h as u64) as u32) & !1, nh)
}

/// Sends one BGRA picture to WebRTC (and, every few frames, to our preview).
fn send_bgra(source: &NativeVideoSource, bgra: &[u8], stride: u32, w: u32, h: u32, max_h: u32, start: Instant, preview: Option<&mut Preview>) {
    let (w, h) = (w & !1, h & !1); // I420 wants even sizes
    if w == 0 || h == 0 {
        return;
    }
    let mut buf = I420Buffer::new(w, h);
    let (sy, su, sv) = buf.strides();
    let (y, u, v) = buf.data_mut();
    yuv_helper::argb_to_i420(bgra, stride, y, sy, u, su, v, sv, w as i32, h as i32);
    let (nw, nh) = fit(w, h, max_h);
    if (nw, nh) != (w, h) && nw > 0 && nh > 0 {
        buf = buf.scale(nw as i32, nh as i32);
    }
    if let Some(p) = preview {
        p.offer(&mut buf);
    }
    let mut frame = VideoFrame::new(VideoRotation::VideoRotation0, buf);
    frame.timestamp_us = start.elapsed().as_micros() as i64;
    source.capture_frame(&frame);
}

/// Our own preview: exactly what is sent (after the resolution limit), up to 30 times a second
/// and up to 1920 pixels wide, scaled with libyuv's filtering so it's sharp when shown big.
struct Preview {
    frames: Frames,
    key: String,
    last: Instant,
}

impl Preview {
    fn new(frames: Frames, key: String) -> Self {
        Preview { frames, key, last: Instant::now() - Duration::from_secs(1) }
    }
    fn offer(&mut self, buf: &mut I420Buffer) {
        let (w, h) = (buf.width(), buf.height());
        if self.last.elapsed() < Duration::from_millis(33) || w == 0 || h == 0 {
            return;
        }
        self.last = Instant::now();
        let small;
        let src: &I420Buffer = if w > 1920 {
            let ph = ((h as u64 * 1920 / w as u64) as u32) & !1;
            small = buf.scale(1920, ph as i32);
            &small
        } else {
            buf
        };
        let (pw, ph) = (src.width(), src.height());
        let (sy, su, sv) = src.strides();
        let (y, u, v) = src.data();
        let mut out = vec![0u8; (pw * ph * 4) as usize];
        yuv_helper::i420_to_argb(y, sy, u, su, v, sv, &mut out, pw * 4, pw as i32, ph as i32);
        self.frames.put(&self.key, pw, ph, out);
    }
}

/// Cameras by name, for Settings.
pub fn cameras() -> Vec<String> {
    use nokhwa::utils::ApiBackend;
    nokhwa::query(ApiBackend::Auto).unwrap_or_default().iter().map(|c| c.human_name()).collect()
}

/// The camera size closest to `height` (360, 720 or 1080).
fn camera_size(height: u32) -> (u32, u32) {
    match height {
        0..=480 => (640, 360),
        481..=900 => (1280, 720),
        _ => (1920, 1080),
    }
}

pub fn camera(frames: Frames, key: String, device: String, limits: Arc<Limits>, stop: Arc<AtomicBool>, ended: Box<dyn FnOnce() + Send>) -> Result<NativeVideoSource, String> {
    let source = NativeVideoSource::new(VideoResolution { width: 1280, height: 720 }, false);
    let src = source.clone();
    let (ok_tx, ok_rx) = mpsc::sync_channel::<Result<(), String>>(1);
    std::thread::Builder::new().name("litecord-camera".into()).spawn(move || {
        let mut preview = Preview::new(frames, key);
        let start = Instant::now();
        if std::env::var_os("LITECORD_FAKE_CAMERA").is_some() {
            let _ = ok_tx.send(Ok(()));
            let (w, h) = (640u32, 360u32);
            let mut n = 0u32;
            while !stop.load(Ordering::SeqCst) {
                let mut px = vec![0u8; (w * h * 4) as usize];
                for y in 0..h {
                    for x in 0..w {
                        let i = ((y * w + x) * 4) as usize;
                        let bar = ((x + n * 4) / 80) % 6;
                        let c = [[40, 40, 230], [40, 200, 40], [230, 60, 40], [40, 220, 230], [230, 40, 200], [230, 230, 40]][bar as usize];
                        px[i] = c[0];
                        px[i + 1] = c[1];
                        px[i + 2] = (c[2] as u32 * (h - y) / h) as u8;
                        px[i + 3] = 255;
                    }
                }
                send_bgra(&src, &px, w * 4, w, h, limits.max_h.load(Ordering::Relaxed), start, Some(&mut preview));
                n += 1;
                std::thread::sleep(limits.frame_time());
            }
            return;
        }
        use nokhwa::utils::{ApiBackend, CameraFormat, CameraIndex, FrameFormat, RequestedFormat, RequestedFormatType, Resolution};
        let cams = nokhwa::query(ApiBackend::Auto).unwrap_or_default();
        let Some(info) = cams.iter().find(|c| !device.is_empty() && c.human_name() == device).or(cams.first()) else {
            let _ = ok_tx.send(Err("no camera found".into()));
            return;
        };
        let (cw, ch) = camera_size(limits.max_h.load(Ordering::Relaxed));
        let want = RequestedFormat::with_formats(
            RequestedFormatType::Closest(CameraFormat::new(Resolution::new(cw, ch), FrameFormat::MJPEG, limits.fps.load(Ordering::Relaxed))),
            &[FrameFormat::MJPEG, FrameFormat::YUYV, FrameFormat::NV12, FrameFormat::RAWRGB, FrameFormat::RAWBGR, FrameFormat::GRAY],
        );
        let mut cam = match nokhwa::Camera::new(info.index().clone(), want).and_then(|mut c| c.open_stream().map(|_| c)) {
            Ok(c) => c,
            Err(e) => {
                let _ = ok_tx.send(Err(e.to_string()));
                return;
            }
        };
        let _ = ok_tx.send(Ok(()));
        let _ = CameraIndex::Index(0);
        let mut fails = 0;
        while !stop.load(Ordering::SeqCst) {
            let fmt = cam.camera_format();
            let (w, h) = (fmt.resolution().width(), fmt.resolution().height());
            let raw = match cam.frame_raw() {
                Ok(r) => r.into_owned(),
                Err(_) => {
                    fails += 1;
                    if fails > 50 {
                        break; // unplugged
                    }
                    std::thread::sleep(Duration::from_millis(20));
                    continue;
                }
            };
            fails = 0;
            if let Some(bgra) = to_bgra(&raw, fmt.format(), w, h) {
                send_bgra(&src, &bgra, w * 4, w, h, limits.max_h.load(Ordering::Relaxed), start, Some(&mut preview));
            }
        }
        let _ = cam.stop_stream();
        if !stop.load(Ordering::SeqCst) {
            ended();
        }
    }).map_err(|e| e.to_string())?;
    match ok_rx.recv_timeout(Duration::from_secs(10)) {
        Ok(Ok(())) => Ok(source),
        Ok(Err(e)) => Err(e),
        Err(_) => Err("the camera didn't answer".into()),
    }
}

/// Camera bytes to BGRA.
fn to_bgra(raw: &[u8], fmt: nokhwa::utils::FrameFormat, w: u32, h: u32) -> Option<Vec<u8>> {
    use nokhwa::utils::FrameFormat as F;
    let n = (w * h) as usize;
    let mut out = vec![255u8; n * 4];
    match fmt {
        F::MJPEG => {
            let img = image::load_from_memory_with_format(raw, image::ImageFormat::Jpeg).ok()?.to_rgb8();
            if img.width() != w || img.height() != h {
                return None;
            }
            for (i, p) in img.pixels().enumerate() {
                out[i * 4] = p[2];
                out[i * 4 + 1] = p[1];
                out[i * 4 + 2] = p[0];
            }
        }
        F::RAWRGB | F::RAWBGR => {
            if raw.len() < n * 3 {
                return None;
            }
            for i in 0..n {
                let (r, g, b) = if fmt == F::RAWRGB { (raw[i * 3], raw[i * 3 + 1], raw[i * 3 + 2]) } else { (raw[i * 3 + 2], raw[i * 3 + 1], raw[i * 3]) };
                out[i * 4] = b;
                out[i * 4 + 1] = g;
                out[i * 4 + 2] = r;
            }
        }
        F::GRAY => {
            for i in 0..n.min(raw.len()) {
                out[i * 4] = raw[i];
                out[i * 4 + 1] = raw[i];
                out[i * 4 + 2] = raw[i];
            }
        }
        F::YUYV | F::NV12 => {
            // Through I420 with libyuv.
            let mut i420 = I420Buffer::new(w, h);
            let (sy, su, sv) = i420.strides();
            {
                let (y, u, v) = i420.data_mut();
                if fmt == F::NV12 {
                    if raw.len() < n * 3 / 2 {
                        return None;
                    }
                    yuv_helper::nv12_to_i420(&raw[..n], w, &raw[n..], w, y, sy, u, su, v, sv, w as i32, h as i32);
                } else {
                    if raw.len() < n * 2 {
                        return None;
                    }
                    // YUYV: Y0 U Y1 V for each pair of pixels.
                    for row in 0..h as usize {
                        for pair in 0..(w as usize / 2) {
                            let i = row * w as usize * 2 + pair * 4;
                            y[row * sy as usize + pair * 2] = raw[i];
                            y[row * sy as usize + pair * 2 + 1] = raw[i + 2];
                            if row % 2 == 0 {
                                u[row / 2 * su as usize + pair] = raw[i + 1];
                                v[row / 2 * sv as usize + pair] = raw[i + 3];
                            }
                        }
                    }
                }
            }
            let (y, u, v) = i420.data();
            yuv_helper::i420_to_argb(y, sy, u, su, v, sv, &mut out, w * 4, w as i32, h as i32);
        }
    }
    Some(out)
}

/// Is this a Wayland session (where screen capture goes through the portal)?
#[cfg(target_os = "linux")]
fn wayland() -> bool {
    std::env::var_os("WAYLAND_DISPLAY").is_some() || std::env::var("XDG_SESSION_TYPE").map(|s| s == "wayland").unwrap_or(false)
}

pub fn screen(frames: Frames, key: String, source_id: Option<u64>, limits: Arc<Limits>, stop: Arc<AtomicBool>, ended: Box<dyn FnOnce() + Send>) -> Result<NativeVideoSource, String> {
    use libwebrtc::desktop_capturer::{CaptureError, DesktopCaptureSourceType, DesktopCapturer, DesktopCapturerOptions};
    let source = NativeVideoSource::new(VideoResolution { width: 1920, height: 1080 }, true);
    let src = source.clone();
    let (ok_tx, ok_rx) = mpsc::sync_channel::<Result<(), String>>(1);
    std::thread::Builder::new().name("litecord-screen".into()).spawn(move || {
        #[cfg(target_os = "linux")]
        let kind = if wayland() { DesktopCaptureSourceType::Generic } else { DesktopCaptureSourceType::Screen };
        #[cfg(not(target_os = "linux"))]
        let kind = DesktopCaptureSourceType::Screen;
        let mut opts = DesktopCapturerOptions::new(kind);
        opts.set_include_cursor(true);
        let Some(mut cap) = DesktopCapturer::new(opts) else {
            let _ = ok_tx.send(Err("screen capture isn't available here".into()));
            return;
        };
        // On Wayland the portal asks the user; elsewhere pick the screen asked for, or the first.
        let list = cap.get_source_list();
        let pick = match source_id {
            Some(id) => list.into_iter().find(|s| s.id() == id),
            None => list.into_iter().next(),
        };
        let start = Instant::now();
        let failed = Arc::new(AtomicBool::new(false));
        let f2 = failed.clone();
        let mut preview = Preview::new(frames, key);
        let lim = limits.clone();
        cap.start_capture(pick, move |r| match r {
            Ok(frame) => {
                let (w, h) = (frame.width() as u32, frame.height() as u32);
                send_bgra(&src, frame.data(), frame.stride(), w, h, lim.max_h.load(Ordering::Relaxed), start, Some(&mut preview));
            }
            Err(CaptureError::Permanent) => f2.store(true, Ordering::SeqCst),
            Err(_) => {}
        });
        let _ = ok_tx.send(Ok(()));
        // Paced to the frame rate picked, on a steady clock so 60 FPS really is 60.
        let mut next = Instant::now();
        while !stop.load(Ordering::SeqCst) && !failed.load(Ordering::SeqCst) {
            cap.capture_frame();
            next += limits.frame_time();
            let now = Instant::now();
            if next > now {
                std::thread::sleep(next - now);
            } else {
                next = now;
            }
        }
        drop(cap);
        if !stop.load(Ordering::SeqCst) {
            ended(); // the user stopped sharing from the system, or the screen went away
        }
    }).map_err(|e| e.to_string())?;
    match ok_rx.recv_timeout(Duration::from_secs(60)) {
        Ok(Ok(())) => Ok(source),
        Ok(Err(e)) => Err(e),
        Err(_) => Err("no answer from screen capture".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::{fit, I420Buffer, Preview};
    use crate::call::video::Frames;

    #[test]
    fn preview_is_sharp_and_paced() {
        let frames = Frames::default();
        let mut p = Preview::new(frames.clone(), "me:screen".into());
        let mut buf = I420Buffer::new(2560, 1440);
        p.offer(&mut buf);
        let img = frames.get("me:screen").expect("a preview");
        assert_eq!((img.width, img.height), (1920, 1080));
        assert_eq!(img.take_pixels().len(), 1920 * 1080 * 4);
        // A second frame right away is skipped (at most 30 previews a second).
        p.offer(&mut buf);
        assert_eq!(frames.get("me:screen").unwrap().seq, 1);
    }

    #[test]
    fn fits_to_height() {
        assert_eq!(fit(2560, 1440, 1080), (1920, 1080));
        assert_eq!(fit(1920, 1080, 1440), (1920, 1080));
        assert_eq!(fit(3841, 2161, 0), (3840, 2160));
    }
}

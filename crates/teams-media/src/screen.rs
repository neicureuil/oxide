//! Screen and window sharing with libwebrtc's desktop capturer, at 15 fps on the sharing line.
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use libwebrtc::desktop_capturer::{
    CaptureError, CaptureSource, DesktopCaptureSourceType, DesktopCapturer, DesktopCapturerOptions,
    DesktopFrame,
};
use libwebrtc::native::yuv_helper;
use libwebrtc::peer_connection_factory::PeerConnectionFactory;
use libwebrtc::peer_connection_factory::native::PeerConnectionFactoryExt;
use libwebrtc::prelude::RtcVideoTrack;
use libwebrtc::video_frame::{I420Buffer, VideoBuffer, VideoFrame, VideoRotation};
use libwebrtc::video_source::VideoResolution;
use libwebrtc::video_source::native::NativeVideoSource;

use crate::camera::{CaptureThread, Failed, SizeChanged};
use crate::video::{Sinks, VideoSlot};

const FRAME_INTERVAL: Duration = Duration::from_millis(1000 / 15);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ScreenSourceKind {
    Screen,
    Window,
}

/// A screen or a window the user can share.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct ScreenSource {
    pub kind: ScreenSourceKind,
    pub id: u64,
    pub title: String,
}

fn options(kind: ScreenSourceKind) -> DesktopCapturerOptions {
    let mut options = DesktopCapturerOptions::new(match kind {
        ScreenSourceKind::Screen => DesktopCaptureSourceType::Screen,
        ScreenSourceKind::Window => DesktopCaptureSourceType::Window,
    });
    options.set_include_cursor(true);
    options
}

/// The screens or windows that can be shared now. Needs the screen recording permission on macOS.
pub fn screen_sources(kind: ScreenSourceKind) -> Vec<ScreenSource> {
    let Some(capturer) = DesktopCapturer::new(options(kind)) else {
        return Vec::new();
    };
    capturer
        .get_source_list()
        .into_iter()
        .map(|s| ScreenSource {
            kind,
            id: s.id(),
            title: s.title(),
        })
        .collect()
}

impl ScreenSource {
    /// One frame of the source scaled to fit `max_width`, as RGBA rows: (width, height, pixels). For the picker.
    pub fn thumbnail(&self, max_width: u32) -> Option<(u32, u32, Vec<u8>)> {
        let mut capturer = DesktopCapturer::new(options(self.kind))?;
        let source = find(&capturer, self.id)?;
        let (tx, rx) = mpsc::channel();
        capturer.start_capture(Some(source), move |result| {
            if let Ok(frame) = result {
                let _ = tx.send(scaled_rgba(&frame, max_width));
            }
        });
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            capturer.capture_frame();
            if let Ok(thumbnail) = rx.recv_timeout(Duration::from_millis(100)) {
                return thumbnail;
            }
        }
        None
    }
}

fn find(capturer: &DesktopCapturer, id: u64) -> Option<CaptureSource> {
    capturer
        .get_source_list()
        .into_iter()
        .find(|s| s.id() == id)
}

fn scaled_rgba(frame: &DesktopFrame, max_width: u32) -> Option<(u32, u32, Vec<u8>)> {
    let mut i420 = to_i420(frame)?;
    let (width, height) = (i420.width(), i420.height());
    let scale = (f64::from(max_width) / f64::from(width)).min(1.0);
    let (w, h) = (
        ((f64::from(width) * scale) as u32).max(2) & !1,
        ((f64::from(height) * scale) as u32).max(2) & !1,
    );
    let small = i420.scale(w as i32, h as i32);
    let (y, u, v) = small.data();
    let (sy, su, sv) = small.strides();
    let mut rgba = vec![0u8; (w * h * 4) as usize];
    // libyuv's "ABGR" is R, G, B, A in memory.
    yuv_helper::i420_to_abgr(y, sy, u, su, v, sv, &mut rgba, w * 4, w as i32, h as i32);
    Some((w, h, rgba))
}

/// Desktop frames are BGRA in memory, libyuv's "ARGB".
fn to_i420(frame: &DesktopFrame) -> Option<I420Buffer> {
    let (width, height) = (frame.width(), frame.height());
    if width <= 0 || height <= 0 {
        return None;
    }
    let mut out = I420Buffer::new(width as u32, height as u32);
    let (sy, su, sv) = out.strides();
    let (y, u, v) = out.data_mut();
    yuv_helper::argb_to_i420(
        frame.data(),
        frame.stride(),
        y,
        sy,
        u,
        su,
        v,
        sv,
        width,
        height,
    );
    Some(out)
}

pub(crate) struct ScreenCapture(CaptureThread);

impl ScreenCapture {
    /// Captures `source`, else the main screen. `None` when capturing isn't allowed or possible.
    pub fn start(
        factory: &PeerConnectionFactory,
        source: Option<ScreenSource>,
        sinks: Sinks,
        size_changed: SizeChanged,
        failed: Failed,
    ) -> Option<(ScreenCapture, RtcVideoTrack)> {
        let kind = source.as_ref().map_or(ScreenSourceKind::Screen, |s| s.kind);
        let mut capturer = DesktopCapturer::new(options(kind))?;
        let chosen = match &source {
            Some(s) => Some(find(&capturer, s.id)?),
            None => capturer.get_source_list().into_iter().next(),
        };
        log::info!("screen share kind={kind:?} found={}", chosen.is_some());
        let video = NativeVideoSource::new(
            VideoResolution {
                width: 1920,
                height: 1080,
            },
            true,
        );
        let track = factory.create_video_track("screen0", video.clone());
        let permanent_failure = Arc::new(AtomicBool::new(false));
        {
            let permanent_failure = permanent_failure.clone();
            let mut size = (0, 0);
            capturer.start_capture(chosen, move |result| match result {
                Ok(frame) => {
                    let Some(buffer) = to_i420(&frame) else {
                        return;
                    };
                    let frame_size = (buffer.width(), buffer.height());
                    if size != frame_size {
                        size = frame_size;
                        size_changed(frame_size.0, frame_size.1);
                    }
                    sinks.deliver(&VideoSlot::Screen, &buffer, VideoRotation::VideoRotation0);
                    video.capture_frame(&VideoFrame::new(VideoRotation::VideoRotation0, buffer));
                }
                Err(CaptureError::Permanent) => permanent_failure.store(true, Ordering::Relaxed),
                Err(CaptureError::Temporary) => {}
            });
        }
        let thread = CaptureThread::spawn("teams-media-screen", move |stop| {
            while !stop.load(Ordering::Relaxed) {
                let started = Instant::now();
                capturer.capture_frame();
                if permanent_failure.load(Ordering::Relaxed) {
                    log::warn!("screen capture failed");
                    failed();
                    return;
                }
                std::thread::sleep(FRAME_INTERVAL.saturating_sub(started.elapsed()));
            }
        })?;
        Some((ScreenCapture(thread), track))
    }

    pub fn stop(self) {
        self.0.stop();
    }
}

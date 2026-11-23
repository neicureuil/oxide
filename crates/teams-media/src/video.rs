//! Video frames handed to the app: remote lines by mid, our camera and our screen.
use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::{Arc, Mutex};

use libwebrtc::prelude::RtcVideoTrack;
use libwebrtc::video_frame::{VideoBuffer, VideoRotation};
use libwebrtc::video_stream::native::NativeVideoStream;
use tokio_stream::StreamExt;

/// Where a frame comes from.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum VideoSlot {
    /// Our camera, before encoding.
    Camera,
    /// Our shared screen, before encoding.
    Screen,
    /// A remote video line (WebRTC mid): a mixer camera slot or the shared screen.
    Remote(String),
}

/// The pixels of one frame, borrowed for the duration of [`VideoSink::on_frame`].
#[derive(Debug)]
pub enum VideoPlanes<'a> {
    I420 {
        y: &'a [u8],
        u: &'a [u8],
        v: &'a [u8],
        stride_y: u32,
        stride_u: u32,
        stride_v: u32,
    },
    Nv12 {
        y: &'a [u8],
        uv: &'a [u8],
        stride_y: u32,
        stride_uv: u32,
    },
    /// macOS: the `CVPixelBufferRef` of the hardware decoder or the camera; retain it to keep it.
    CvPixelBuffer(*mut c_void),
}

#[derive(Debug)]
pub struct VideoFrame<'a> {
    pub width: u32,
    pub height: u32,
    /// Clockwise degrees to rotate before showing: 0, 90, 180 or 270.
    pub rotation: u16,
    pub planes: VideoPlanes<'a>,
}

/// Shows frames. Called on an engine thread for every frame; return quickly (drop frames when busy).
pub trait VideoSink: Send + Sync {
    fn on_frame(&self, frame: &VideoFrame<'_>);
}

impl<F: Fn(&VideoFrame<'_>) + Send + Sync> VideoSink for F {
    fn on_frame(&self, frame: &VideoFrame<'_>) {
        self(frame)
    }
}

/// The sinks the app registered, shared with the frame pumps.
#[derive(Clone, Default)]
pub(crate) struct Sinks(Arc<Mutex<HashMap<VideoSlot, Arc<dyn VideoSink>>>>);

impl Sinks {
    pub fn set(&self, slot: VideoSlot, sink: Option<Arc<dyn VideoSink>>) {
        let mut sinks = self.0.lock().unwrap();
        match sink {
            Some(sink) => sinks.insert(slot, sink),
            None => sinks.remove(&slot),
        };
    }

    /// Hands `buffer` to the slot's sink, if any, converting only what the sink can't take as is.
    pub fn deliver(&self, slot: &VideoSlot, buffer: &dyn VideoBuffer, rotation: VideoRotation) {
        let sink = self.0.lock().unwrap().get(slot).cloned();
        if let Some(sink) = sink {
            deliver(&*sink, buffer, rotation);
        }
    }
}

/// macOS: media threads have no autorelease pool, so each frame's ObjC wrappers would leak their buffer.
#[cfg(target_os = "macos")]
fn with_autorelease_pool<R>(f: impl FnOnce() -> R) -> R {
    unsafe extern "C" {
        fn objc_autoreleasePoolPush() -> *mut c_void;
        fn objc_autoreleasePoolPop(pool: *mut c_void);
    }
    struct Pool(*mut c_void);
    impl Drop for Pool {
        fn drop(&mut self) {
            unsafe { objc_autoreleasePoolPop(self.0) }
        }
    }
    let _pool = Pool(unsafe { objc_autoreleasePoolPush() });
    f()
}

#[cfg(not(target_os = "macos"))]
fn with_autorelease_pool<R>(f: impl FnOnce() -> R) -> R {
    f()
}

fn deliver(sink: &dyn VideoSink, buffer: &dyn VideoBuffer, rotation: VideoRotation) {
    with_autorelease_pool(|| deliver_frame(sink, buffer, rotation))
}

fn deliver_frame(sink: &dyn VideoSink, buffer: &dyn VideoBuffer, rotation: VideoRotation) {
    let rotation = match rotation {
        VideoRotation::VideoRotation0 => 0,
        VideoRotation::VideoRotation90 => 90,
        VideoRotation::VideoRotation180 => 180,
        VideoRotation::VideoRotation270 => 270,
    };
    let (width, height) = (buffer.width(), buffer.height());
    #[cfg(target_os = "macos")]
    if let Some(native) = buffer.as_native() {
        let pixels = native.get_cv_pixel_buffer();
        if !pixels.is_null() {
            let planes = VideoPlanes::CvPixelBuffer(pixels);
            sink.on_frame(&VideoFrame {
                width,
                height,
                rotation,
                planes,
            });
            return;
        }
    }
    if let Some(nv12) = buffer.as_nv12() {
        let (y, uv) = nv12.data();
        let (stride_y, stride_uv) = nv12.strides();
        let planes = VideoPlanes::Nv12 {
            y,
            uv,
            stride_y,
            stride_uv,
        };
        sink.on_frame(&VideoFrame {
            width,
            height,
            rotation,
            planes,
        });
        return;
    }
    let converted;
    let i420 = match buffer.as_i420() {
        Some(i420) => i420,
        None => {
            converted = libwebrtc::video_frame::to_i420(buffer);
            &converted
        }
    };
    let (y, u, v) = i420.data();
    let (stride_y, stride_u, stride_v) = i420.strides();
    let planes = VideoPlanes::I420 {
        y,
        u,
        v,
        stride_y,
        stride_u,
        stride_v,
    };
    sink.on_frame(&VideoFrame {
        width,
        height,
        rotation,
        planes,
    });
}

/// Forwards a track's frames to its slot's sink until it ends, reporting each new frame size.
pub(crate) fn pump(
    track: RtcVideoTrack,
    slot: VideoSlot,
    sinks: Sinks,
    size_changed: Option<crate::camera::SizeChanged>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut stream = NativeVideoStream::new(track);
        let mut size = (0, 0);
        while let Some(frame) = stream.next().await {
            let frame_size = (frame.buffer.width(), frame.buffer.height());
            if frame_size != size {
                size = frame_size;
                if let Some(changed) = &size_changed {
                    changed(size.0, size.1);
                }
            }
            sinks.deliver(&slot, frame.buffer.as_ref(), frame.rotation);
        }
    })
}

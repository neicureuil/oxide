//! The camera: libwebrtc's AVFoundation capturer on macOS, nokhwa (Media Foundation, V4L2) elsewhere.
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::JoinHandle;

use libwebrtc::peer_connection_factory::PeerConnectionFactory;
use libwebrtc::peer_connection_factory::native::PeerConnectionFactoryExt;
use libwebrtc::prelude::RtcVideoTrack;
use libwebrtc::video_frame::{I420Buffer, VideoFrame, VideoRotation};
use libwebrtc::video_source::VideoResolution;
use libwebrtc::video_source::native::NativeVideoSource;
use nokhwa::utils::{ApiBackend, CameraIndex};

use crate::DeviceChoice;
use crate::video::{Sinks, VideoSlot};

/// Teams' camera lines take up to 3600 macroblocks: 1280×720.
const PREFERRED_WIDTH: u32 = 1280;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CameraDevice {
    /// AVFoundation unique id, Media Foundation symbolic link, V4L2 device path.
    pub id: String,
    pub name: String,
}

pub fn cameras() -> Vec<CameraDevice> {
    match nokhwa::query(ApiBackend::Auto) {
        Ok(found) => found
            .iter()
            .map(|info| CameraDevice {
                id: device_id(info.misc(), info.index()),
                name: info.human_name(),
            })
            .collect(),
        Err(e) => {
            log::warn!("cameras: {e}");
            Vec::new()
        }
    }
}

fn device_id(misc: String, index: &CameraIndex) -> String {
    if misc.is_empty() {
        index.to_string()
    } else {
        misc
    }
}

pub(crate) type SizeChanged = Box<dyn Fn(u32, u32) + Send>;
pub(crate) type Failed = Box<dyn Fn() + Send + Sync>;

/// A capture loop on its own thread, stopped through a shared flag.
pub(crate) struct CaptureThread {
    stop: Arc<AtomicBool>,
    handle: JoinHandle<()>,
}

impl CaptureThread {
    pub fn spawn(name: &str, run: impl FnOnce(&AtomicBool) + Send + 'static) -> Option<Self> {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let handle = std::thread::Builder::new()
            .name(name.into())
            .spawn(move || run(&flag))
            .ok()?;
        Some(Self { stop, handle })
    }

    pub fn stop(self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.handle.join();
    }
}

pub(crate) struct Camera {
    thread: Option<CaptureThread>,
    #[cfg(target_os = "macos")]
    mac: Option<libwebrtc::native::camera_mac::MacCamera>,
    /// macOS: preview and frame size read back from the track, the capture being native.
    preview: Option<tokio::task::JoinHandle<()>>,
}

impl Camera {
    fn threaded(thread: CaptureThread) -> Self {
        Self {
            thread: Some(thread),
            #[cfg(target_os = "macos")]
            mac: None,
            preview: None,
        }
    }

    #[cfg(target_os = "macos")]
    pub fn start(
        factory: &PeerConnectionFactory,
        choice: Option<&DeviceChoice>,
        sinks: Sinks,
        size_changed: SizeChanged,
        failed: Failed,
    ) -> Option<(Camera, RtcVideoTrack)> {
        // nokhwa can't open the built-in camera's formats on macOS; frames stay CVPixelBuffers here.
        let source = NativeVideoSource::new_external(VideoResolution {
            width: PREFERRED_WIDTH,
            height: 720,
        });
        let (id, name) = choice.map_or((String::new(), String::new()), |c| {
            (c.id.clone(), c.name.clone())
        });
        let mac = libwebrtc::native::camera_mac::MacCamera::start(&source, &id, &name, failed)?;
        log::info!("camera started (AVFoundation)");
        let track = factory.create_video_track("video0", source);
        let preview =
            crate::video::pump(track.clone(), VideoSlot::Camera, sinks, Some(size_changed));
        let camera = Camera {
            thread: None,
            mac: Some(mac),
            preview: Some(preview),
        };
        Some((camera, track))
    }

    #[cfg(not(target_os = "macos"))]
    pub fn start(
        factory: &PeerConnectionFactory,
        choice: Option<&DeviceChoice>,
        sinks: Sinks,
        size_changed: SizeChanged,
        failed: Failed,
    ) -> Option<(Camera, RtcVideoTrack)> {
        let found = nokhwa::query(ApiBackend::Auto).ok()?;
        let info = choice
            .and_then(|c| {
                found
                    .iter()
                    .find(|i| device_id(i.misc(), i.index()) == c.id)
                    .or_else(|| found.iter().find(|i| i.human_name() == c.name))
            })
            .or_else(|| found.first())?;
        let index = info.index().clone();
        log::info!("camera found");
        let source = NativeVideoSource::new(
            VideoResolution {
                width: PREFERRED_WIDTH,
                height: 720,
            },
            false,
        );
        let track = factory.create_video_track("video0", source.clone());
        let thread = CaptureThread::spawn("teams-media-camera", move |stop| {
            if let Err(e) = nokhwa_capture::capture(index, &source, &sinks, stop, &size_changed) {
                log::warn!("camera: {e}");
                if !stop.load(Ordering::Relaxed) {
                    failed();
                }
            }
        })?;
        Some((Camera::threaded(thread), track))
    }

    /// A moving test pattern at 640×360, 15 fps, for engines without devices.
    pub fn synthetic(
        factory: &PeerConnectionFactory,
        _choice: Option<&DeviceChoice>,
        sinks: Sinks,
        size_changed: SizeChanged,
        _failed: Failed,
    ) -> Option<(Camera, RtcVideoTrack)> {
        const W: u32 = 640;
        const H: u32 = 360;
        let source = NativeVideoSource::new(
            VideoResolution {
                width: W,
                height: H,
            },
            false,
        );
        let track = factory.create_video_track("video0", source.clone());
        let thread = CaptureThread::spawn("teams-media-pattern", move |stop| {
            size_changed(W, H);
            let mut tick = 0u32;
            while !stop.load(Ordering::Relaxed) {
                let mut frame = I420Buffer::new(W, H);
                let (sy, su, sv) = frame.strides();
                let (y, u, v) = frame.data_mut();
                for row in 0..H as usize {
                    for x in 0..W as usize {
                        y[row * sy as usize + x] = ((x as u32 + tick * 4) % 256) as u8;
                    }
                }
                u[..su as usize * (H as usize / 2)].fill(128);
                v[..sv as usize * (H as usize / 2)].fill(128);
                sinks.deliver(&VideoSlot::Camera, &frame, VideoRotation::VideoRotation0);
                // macOS: camera-like CVPixelBuffers, so tests cover the native encoder path.
                #[cfg(target_os = "macos")]
                match native_pattern::to_cv_pixel_buffer(&frame) {
                    Some(native) => source
                        .capture_frame(&VideoFrame::new(VideoRotation::VideoRotation0, native)),
                    None => {
                        source.capture_frame(&VideoFrame::new(VideoRotation::VideoRotation0, frame))
                    }
                };
                #[cfg(not(target_os = "macos"))]
                source.capture_frame(&VideoFrame::new(VideoRotation::VideoRotation0, frame));
                tick = tick.wrapping_add(1);
                std::thread::sleep(std::time::Duration::from_millis(66));
            }
        })?;
        Some((Camera::threaded(thread), track))
    }

    pub fn stop(mut self) {
        if let Some(thread) = self.thread.take() {
            thread.stop();
        }
        #[cfg(target_os = "macos")]
        drop(self.mac.take());
        if let Some(preview) = self.preview.take() {
            preview.abort();
        }
    }
}

/// Capture through nokhwa: Windows and Linux (and the tests of its conversions everywhere).
#[cfg(any(test, not(target_os = "macos")))]
#[cfg_attr(target_os = "macos", allow(dead_code))]
mod nokhwa_capture {
    use std::sync::atomic::{AtomicBool, Ordering};

    use libwebrtc::native::yuv_helper;
    use libwebrtc::video_frame::{I420Buffer, VideoFrame, VideoRotation};
    use libwebrtc::video_source::native::NativeVideoSource;
    use nokhwa::Camera as Device;
    use nokhwa::pixel_format::RgbFormat;
    use nokhwa::utils::{
        CameraFormat, CameraIndex, FrameFormat, RequestedFormat, RequestedFormatType,
    };

    use super::{PREFERRED_WIDTH, SizeChanged};
    use crate::video::{Sinks, VideoSlot};

    const MAX_FPS: u32 = 30;

    pub(super) fn capture(
        index: CameraIndex,
        source: &NativeVideoSource,
        sinks: &Sinks,
        stop: &AtomicBool,
        size_changed: &SizeChanged,
    ) -> Result<(), nokhwa::NokhwaError> {
        let mut device = Device::new(
            index,
            RequestedFormat::new::<RgbFormat>(RequestedFormatType::None),
        )?;
        if let Some(format) = pick_format(device.compatible_camera_formats().unwrap_or_default()) {
            log::info!("camera format {format}");
            device.set_camera_requset(RequestedFormat::new::<RgbFormat>(
                RequestedFormatType::Exact(format),
            ))?;
        }
        device.open_stream()?;
        let mut size = (0, 0);
        while !stop.load(Ordering::Relaxed) {
            let frame = device.frame()?;
            let resolution = frame.resolution();
            let (width, height) = (resolution.width(), resolution.height());
            let Some(buffer) = to_i420(&frame, width, height) else {
                continue;
            };
            if size != (width, height) {
                size = (width, height);
                size_changed(width, height);
            }
            sinks.deliver(&VideoSlot::Camera, &buffer, VideoRotation::VideoRotation0);
            source.capture_frame(&VideoFrame::new(VideoRotation::VideoRotation0, buffer));
        }
        device.stop_stream()
    }

    /// 720p when available, else the largest format up to 1280 px wide; raw YUV before MJPEG (no decoding).
    fn pick_format(formats: Vec<CameraFormat>) -> Option<CameraFormat> {
        let rank = |f: &CameraFormat| match f.format() {
            FrameFormat::NV12 => 0,
            FrameFormat::YUYV => 1,
            FrameFormat::MJPEG => 2,
            _ => 3,
        };
        // Closest to 30 fps, never above when there's a choice.
        let fps = |f: &CameraFormat| {
            if f.frame_rate() > MAX_FPS {
                100 + f.frame_rate()
            } else {
                MAX_FPS - f.frame_rate()
            }
        };
        let fitting: Vec<&CameraFormat> = formats
            .iter()
            .filter(|f| rank(f) < 3 && f.width() <= PREFERRED_WIDTH)
            .collect();
        let best_width = fitting.iter().map(|f| f.width()).max()?;
        fitting
            .into_iter()
            .filter(|f| f.width() == best_width)
            .min_by_key(|f| (rank(f), fps(f)))
            .copied()
    }

    fn to_i420(frame: &nokhwa::Buffer, width: u32, height: u32) -> Option<I420Buffer> {
        let data = frame.buffer();
        let mut out = I420Buffer::new(width, height);
        let (stride_y, stride_u, stride_v) = out.strides();
        let (dst_y, dst_u, dst_v) = out.data_mut();
        let (w, h) = (width as usize, height as usize);
        match frame.source_frame_format() {
            FrameFormat::NV12 => {
                // Rows may be padded: derive the stride from the buffer length.
                let stride = (data.len() * 2 / 3 / h).max(w);
                if data.len() < stride * h * 3 / 2 {
                    return None;
                }
                let (y, uv) = data.split_at(stride * h);
                yuv_helper::nv12_to_i420(
                    y,
                    stride as u32,
                    uv,
                    stride as u32,
                    dst_y,
                    stride_y,
                    dst_u,
                    stride_u,
                    dst_v,
                    stride_v,
                    width as i32,
                    height as i32,
                );
            }
            FrameFormat::YUYV => {
                let stride = (data.len() / h).max(w * 2);
                if data.len() < stride * h {
                    return None;
                }
                yuyv_to_i420(
                    data,
                    stride,
                    w,
                    h,
                    (dst_y, stride_y),
                    (dst_u, stride_u),
                    (dst_v, stride_v),
                );
            }
            _ => {
                let rgb = frame.decode_image::<RgbFormat>().ok()?;
                rgb_to_i420(
                    rgb.as_raw(),
                    w,
                    h,
                    (dst_y, stride_y),
                    (dst_u, stride_u),
                    (dst_v, stride_v),
                );
            }
        }
        Some(out)
    }

    type Plane<'a> = (&'a mut [u8], u32);

    fn yuyv_to_i420(src: &[u8], stride: usize, w: usize, h: usize, y: Plane, u: Plane, v: Plane) {
        let (dst_y, sy) = (y.0, y.1 as usize);
        let (dst_u, su) = (u.0, u.1 as usize);
        let (dst_v, sv) = (v.0, v.1 as usize);
        for row in 0..h {
            let line = &src[row * stride..row * stride + w * 2];
            for x in 0..w {
                dst_y[row * sy + x] = line[x * 2];
            }
            // Chroma from even rows, averaged with the next one.
            if row % 2 == 0 {
                let next = if row + 1 < h {
                    &src[(row + 1) * stride..(row + 1) * stride + w * 2]
                } else {
                    line
                };
                for cx in 0..w.div_ceil(2) {
                    let i = cx * 4;
                    if i + 3 >= line.len() {
                        break;
                    }
                    dst_u[row / 2 * su + cx] =
                        ((u16::from(line[i + 1]) + u16::from(next[i + 1])) / 2) as u8;
                    dst_v[row / 2 * sv + cx] =
                        ((u16::from(line[i + 3]) + u16::from(next[i + 3])) / 2) as u8;
                }
            }
        }
    }

    /// BT.601 limited range, what libwebrtc's encoders expect.
    fn rgb_to_i420(src: &[u8], w: usize, h: usize, y: Plane, u: Plane, v: Plane) {
        let (dst_y, sy) = (y.0, y.1 as usize);
        let (dst_u, su) = (u.0, u.1 as usize);
        let (dst_v, sv) = (v.0, v.1 as usize);
        let px = |x: usize, y: usize| {
            let i = (y * w + x) * 3;
            (
                i32::from(src[i]),
                i32::from(src[i + 1]),
                i32::from(src[i + 2]),
            )
        };
        for row in 0..h {
            for x in 0..w {
                let (r, g, b) = px(x, row);
                dst_y[row * sy + x] = (((66 * r + 129 * g + 25 * b + 128) >> 8) + 16) as u8;
            }
        }
        for cy in 0..h.div_ceil(2) {
            for cx in 0..w.div_ceil(2) {
                let (r, g, b) = px((cx * 2).min(w - 1), (cy * 2).min(h - 1));
                dst_u[cy * su + cx] = (((-38 * r - 74 * g + 112 * b + 128) >> 8) + 128) as u8;
                dst_v[cy * sv + cx] = (((112 * r - 94 * g - 18 * b + 128) >> 8) + 128) as u8;
            }
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn format(w: u32, h: u32, f: FrameFormat, fps: u32) -> CameraFormat {
            CameraFormat::new(nokhwa::utils::Resolution::new(w, h), f, fps)
        }

        #[test]
        fn prefers_720p_raw_yuv() {
            let formats = vec![
                format(1920, 1080, FrameFormat::NV12, 30),
                format(1280, 720, FrameFormat::MJPEG, 30),
                format(1280, 720, FrameFormat::NV12, 30),
                format(640, 480, FrameFormat::YUYV, 30),
            ];
            assert_eq!(
                pick_format(formats),
                Some(format(1280, 720, FrameFormat::NV12, 30))
            );
        }

        #[test]
        fn white_rgb_is_white_yuv() {
            let rgb = vec![255u8; 4 * 2 * 3];
            let (mut y, mut u, mut v) = (vec![0u8; 8], vec![0u8; 2], vec![0u8; 2]);
            rgb_to_i420(&rgb, 4, 2, (&mut y, 4), (&mut u, 2), (&mut v, 2));
            assert!(y.iter().all(|&p| p == 235), "{y:?}");
            assert!(u.iter().chain(&v).all(|&p| p == 128));
        }
    }
}

#[cfg(target_os = "macos")]
mod native_pattern {
    use std::ffi::c_void;

    use libwebrtc::video_frame::I420Buffer;
    use libwebrtc::video_frame::native::NativeBuffer;

    type CFTypeRef = *const c_void;
    const FULL_RANGE_NV12: u32 = u32::from_be_bytes(*b"420f");

    // CoreVideo and CoreFoundation come linked with libwebrtc (webrtc-sys).
    unsafe extern "C" {
        static kCVPixelBufferIOSurfacePropertiesKey: CFTypeRef;
        static kCFTypeDictionaryKeyCallBacks: c_void;
        static kCFTypeDictionaryValueCallBacks: c_void;
        fn CFDictionaryCreate(
            allocator: CFTypeRef,
            keys: *const CFTypeRef,
            values: *const CFTypeRef,
            count: isize,
            key_callbacks: *const c_void,
            value_callbacks: *const c_void,
        ) -> CFTypeRef;
        fn CFRelease(cf: CFTypeRef);
        fn CVPixelBufferCreate(
            allocator: CFTypeRef,
            width: usize,
            height: usize,
            format: u32,
            attributes: CFTypeRef,
            out: *mut *mut c_void,
        ) -> i32;
        fn CVPixelBufferLockBaseAddress(buffer: *mut c_void, flags: u64) -> i32;
        fn CVPixelBufferUnlockBaseAddress(buffer: *mut c_void, flags: u64) -> i32;
        fn CVPixelBufferGetBaseAddressOfPlane(buffer: *mut c_void, plane: usize) -> *mut u8;
        fn CVPixelBufferGetBytesPerRowOfPlane(buffer: *mut c_void, plane: usize) -> usize;
    }

    pub(super) fn to_cv_pixel_buffer(frame: &I420Buffer) -> Option<NativeBuffer> {
        use libwebrtc::video_frame::VideoBuffer;
        let (width, height) = (frame.width() as usize, frame.height() as usize);
        unsafe {
            let empty = CFDictionaryCreate(
                std::ptr::null(),
                std::ptr::null(),
                std::ptr::null(),
                0,
                &kCFTypeDictionaryKeyCallBacks,
                &kCFTypeDictionaryValueCallBacks,
            );
            let keys = [kCVPixelBufferIOSurfacePropertiesKey];
            let values = [empty];
            let attributes = CFDictionaryCreate(
                std::ptr::null(),
                keys.as_ptr(),
                values.as_ptr(),
                1,
                &kCFTypeDictionaryKeyCallBacks,
                &kCFTypeDictionaryValueCallBacks,
            );
            let mut buffer = std::ptr::null_mut();
            let status = CVPixelBufferCreate(
                std::ptr::null(),
                width,
                height,
                FULL_RANGE_NV12,
                attributes,
                &mut buffer,
            );
            CFRelease(attributes);
            CFRelease(empty);
            if status != 0 || buffer.is_null() {
                return None;
            }
            CVPixelBufferLockBaseAddress(buffer, 0);
            let (y, u, v) = frame.data();
            let (sy, su, sv) = frame.strides();
            let dst_y = CVPixelBufferGetBaseAddressOfPlane(buffer, 0);
            let dst_uv = CVPixelBufferGetBaseAddressOfPlane(buffer, 1);
            let stride_y = CVPixelBufferGetBytesPerRowOfPlane(buffer, 0);
            let stride_uv = CVPixelBufferGetBytesPerRowOfPlane(buffer, 1);
            for row in 0..height {
                std::ptr::copy_nonoverlapping(
                    y[row * sy as usize..].as_ptr(),
                    dst_y.add(row * stride_y),
                    width,
                );
            }
            for row in 0..height.div_ceil(2) {
                let line = dst_uv.add(row * stride_uv);
                for x in 0..width.div_ceil(2) {
                    *line.add(2 * x) = u[row * su as usize + x];
                    *line.add(2 * x + 1) = v[row * sv as usize + x];
                }
            }
            CVPixelBufferUnlockBaseAddress(buffer, 0);
            // The native buffer takes this reference over.
            Some(NativeBuffer::from_cv_pixel_buffer(buffer))
        }
    }
}

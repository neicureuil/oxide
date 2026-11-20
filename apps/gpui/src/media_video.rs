//! Video frames to the screen (`VideoLayerView.swift`): a renderer on a WebRTC track (or a camera preview) keeps the
//! latest frame as a full-range NV12 `CVPixelBuffer`, which GPUI's `surface` element draws.
use crate::media_engine::{MediaMsg, Window as Target};
use crate::media_objc::{CGSize, Obj};
use core_foundation::base::{CFType, TCFType};
use core_foundation::dictionary::CFDictionary;
use core_foundation::number::CFNumber;
use core_foundation::string::CFString;
use core_video::pixel_buffer::{
    CVPixelBuffer, CVPixelBufferGetBaseAddressOfPlane, CVPixelBufferGetBytesPerRowOfPlane,
    CVPixelBufferGetHeightOfPlane, CVPixelBufferGetWidthOfPlane, CVPixelBufferLockBaseAddress,
    CVPixelBufferRef, CVPixelBufferUnlockBaseAddress, kCVPixelBufferHeightKey,
    kCVPixelBufferIOSurfacePropertiesKey, kCVPixelBufferPixelFormatTypeKey, kCVPixelBufferWidthKey,
    kCVPixelFormatType_420YpCbCr8BiPlanarFullRange,
    kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
};
use core_video::pixel_buffer_pool::CVPixelBufferPool;
use futures::channel::mpsc::UnboundedSender;
use gpui::{AnyElement, ObjectFit, div, prelude::*, surface};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject};
use objc2::{AnyThread, DefinedClass, define_class, msg_send};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

/// A pixel buffer handed between the WebRTC threads and the UI (CoreVideo buffers are thread-safe).
#[derive(Clone)]
pub struct Frame(pub CVPixelBuffer);
unsafe impl Send for Frame {}
unsafe impl Sync for Frame {}

struct Pool {
    pool: CVPixelBufferPool,
    width: usize,
    height: usize,
}
unsafe impl Send for Pool {}

/// The latest frame of one video, shared by its renderer (WebRTC threads) and the window drawing it.
pub struct FrameSlot {
    pub id: u64,
    latest: Mutex<Option<Frame>>,
    /// A frame waits for the window: newer ones are dropped until it is drawn, as the macOS app does.
    pending: AtomicBool,
    /// Our self-view is shown mirrored.
    mirrored: bool,
    /// Reused NV12 buffers, rebuilt when the resolution changes.
    pool: Mutex<Option<Pool>>,
    /// Frame size, reported when it changes (local tracks scale their encoding to it).
    size: Mutex<(i32, i32)>,
    report_size: bool,
    wake: UnboundedSender<MediaMsg>,
    target: Target,
}

static NEXT_SLOT: AtomicU64 = AtomicU64::new(1);

impl FrameSlot {
    pub fn new(
        mirrored: bool,
        report_size: bool,
        target: Target,
        wake: UnboundedSender<MediaMsg>,
    ) -> Arc<Self> {
        Arc::new(Self {
            id: NEXT_SLOT.fetch_add(1, Ordering::Relaxed),
            latest: Mutex::new(None),
            pending: AtomicBool::new(false),
            mirrored,
            pool: Mutex::new(None),
            size: Mutex::new((0, 0)),
            report_size,
            wake,
            target,
        })
    }

    /// The frame to draw now; lets the next one in.
    pub fn take(&self) -> Option<CVPixelBuffer> {
        let frame = self.latest.lock().unwrap().clone();
        self.pending.store(false, Ordering::Release);
        frame.map(|f| f.0)
    }

    /// Drops the frame kept for redraws (the video stopped).
    pub fn clear(&self) {
        *self.latest.lock().unwrap() = None;
    }

    fn note_size(&self, width: i32, height: i32) {
        if !self.report_size {
            return;
        }
        let mut size = self.size.lock().unwrap();
        if *size != (width, height) {
            *size = (width, height);
            let _ = self.wake.unbounded_send(MediaMsg::FrameSize {
                slot: self.id,
                width,
                height,
            });
        }
    }

    fn publish(&self, frame: CVPixelBuffer) {
        *self.latest.lock().unwrap() = Some(Frame(frame));
        let _ = self.wake.unbounded_send(MediaMsg::Frame(self.target));
    }

    /// Claims the right to publish a frame; false while the last one waits for the window.
    fn claim(&self) -> bool {
        !self.pending.swap(true, Ordering::AcqRel)
    }
    fn release(&self) {
        self.pending.store(false, Ordering::Release);
    }

    fn buffer(&self, width: usize, height: usize) -> Option<CVPixelBuffer> {
        let mut pool = self.pool.lock().unwrap();
        if pool
            .as_ref()
            .is_none_or(|p| p.width != width || p.height != height)
        {
            *pool = make_pool(width, height).map(|pool| Pool {
                pool,
                width,
                height,
            });
        }
        pool.as_ref()?.pool.create_pixel_buffer().ok()
    }

    /// A frame from libwebrtc (`LKRTCVideoFrame`), on its thread.
    fn receive(&self, frame: &AnyObject) {
        let width: i32 = unsafe { msg_send![frame, width] };
        let height: i32 = unsafe { msg_send![frame, height] };
        self.note_size(width, height);
        if !self.claim() {
            return;
        }
        let buffer: Option<Obj> = unsafe { msg_send![frame, buffer] };
        let converted = buffer.and_then(|buffer| self.convert(&buffer));
        match converted {
            Some(pixels) => self.publish(pixels),
            None => self.release(),
        }
    }

    /// A camera preview's frame (`CMSampleBuffer`'s image), on the capture queue.
    pub fn receive_pixels(&self, pixels: CVPixelBufferRef) {
        if pixels.is_null() {
            return;
        }
        let pixels = unsafe { CVPixelBuffer::wrap_under_get_rule(pixels) };
        self.note_size(pixels.get_width() as i32, pixels.get_height() as i32);
        if !self.claim() {
            return;
        }
        match self.copy_nv12(&pixels) {
            Some(out) => self.publish(out),
            None => self.release(),
        }
    }

    fn convert(&self, buffer: &AnyObject) -> Option<CVPixelBuffer> {
        if crate::media_objc::is_kind(buffer, c"LKRTCCVPixelBuffer") {
            let cropping: bool = unsafe { msg_send![buffer, requiresCropping] };
            let pixels: *mut crate::media_objc::CVBuffer =
                unsafe { msg_send![buffer, pixelBuffer] };
            let pixels = pixels as CVPixelBufferRef;
            if !cropping && !pixels.is_null() {
                let pixels = unsafe { CVPixelBuffer::wrap_under_get_rule(pixels) };
                if let Some(out) = self.copy_nv12(&pixels) {
                    return Some(out);
                }
            }
        }
        // Software-decoded frames (and anything else): I420 → NV12.
        let i420: Option<Obj> = unsafe { msg_send![buffer, toI420] };
        let i420 = i420?;
        self.copy_i420(&i420)
    }

    /// NV12 straight from the decoder or camera: full range kept as is unless mirrored, video range expanded.
    fn copy_nv12(&self, pixels: &CVPixelBuffer) -> Option<CVPixelBuffer> {
        let format = pixels.get_pixel_format();
        let full = format == kCVPixelFormatType_420YpCbCr8BiPlanarFullRange;
        if format != kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange && !full {
            return None;
        }
        if full && !self.mirrored {
            return Some(pixels.clone());
        }
        let source = pixels.as_concrete_TypeRef();
        let (width, height) = (pixels.get_width(), pixels.get_height());
        let out = self.buffer(width, height)?;
        let target = out.as_concrete_TypeRef();
        unsafe {
            CVPixelBufferLockBaseAddress(source, 1);
            CVPixelBufferLockBaseAddress(target, 0);
            let planes = [(0usize, 1usize), (1, 2)];
            for (plane, bytes_per_pixel) in planes {
                let rows = CVPixelBufferGetHeightOfPlane(source, plane)
                    .min(CVPixelBufferGetHeightOfPlane(target, plane));
                let columns = CVPixelBufferGetWidthOfPlane(source, plane)
                    .min(CVPixelBufferGetWidthOfPlane(target, plane));
                let from = CVPixelBufferGetBaseAddressOfPlane(source, plane) as *const u8;
                let to = CVPixelBufferGetBaseAddressOfPlane(target, plane) as *mut u8;
                let from_stride = CVPixelBufferGetBytesPerRowOfPlane(source, plane);
                let to_stride = CVPixelBufferGetBytesPerRowOfPlane(target, plane);
                if from.is_null() || to.is_null() {
                    continue;
                }
                let lut = if full {
                    &IDENTITY
                } else if plane == 0 {
                    &LUMA
                } else {
                    &CHROMA
                };
                for row in 0..rows {
                    let src = std::slice::from_raw_parts(
                        from.add(row * from_stride),
                        columns * bytes_per_pixel,
                    );
                    let dst = std::slice::from_raw_parts_mut(
                        to.add(row * to_stride),
                        columns * bytes_per_pixel,
                    );
                    copy_row(src, dst, bytes_per_pixel, self.mirrored, lut);
                }
            }
            CVPixelBufferUnlockBaseAddress(target, 0);
            CVPixelBufferUnlockBaseAddress(source, 1);
        }
        Some(out)
    }

    /// libwebrtc's I420 (video range) into full-range NV12.
    fn copy_i420(&self, i420: &AnyObject) -> Option<CVPixelBuffer> {
        let width: i32 = unsafe { msg_send![i420, width] };
        let height: i32 = unsafe { msg_send![i420, height] };
        let chroma_width: i32 = unsafe { msg_send![i420, chromaWidth] };
        let chroma_height: i32 = unsafe { msg_send![i420, chromaHeight] };
        let data_y: *const u8 = unsafe { msg_send![i420, dataY] };
        let data_u: *const u8 = unsafe { msg_send![i420, dataU] };
        let data_v: *const u8 = unsafe { msg_send![i420, dataV] };
        let stride_y: i32 = unsafe { msg_send![i420, strideY] };
        let stride_u: i32 = unsafe { msg_send![i420, strideU] };
        let stride_v: i32 = unsafe { msg_send![i420, strideV] };
        if width <= 0 || height <= 0 || data_y.is_null() || data_u.is_null() || data_v.is_null() {
            return None;
        }
        let out = self.buffer(width as usize, height as usize)?;
        let target = out.as_concrete_TypeRef();
        unsafe {
            CVPixelBufferLockBaseAddress(target, 0);
            let y = CVPixelBufferGetBaseAddressOfPlane(target, 0) as *mut u8;
            let uv = CVPixelBufferGetBaseAddressOfPlane(target, 1) as *mut u8;
            let y_stride = CVPixelBufferGetBytesPerRowOfPlane(target, 0);
            let uv_stride = CVPixelBufferGetBytesPerRowOfPlane(target, 1);
            if !y.is_null() && !uv.is_null() {
                let (width, height) = (width as usize, height as usize);
                for row in 0..height {
                    let src =
                        std::slice::from_raw_parts(data_y.add(row * stride_y as usize), width);
                    let dst = std::slice::from_raw_parts_mut(y.add(row * y_stride), width);
                    copy_row(src, dst, 1, self.mirrored, &LUMA);
                }
                let (cw, ch) = (chroma_width as usize, chroma_height as usize);
                for row in 0..ch {
                    let u = std::slice::from_raw_parts(data_u.add(row * stride_u as usize), cw);
                    let v = std::slice::from_raw_parts(data_v.add(row * stride_v as usize), cw);
                    let dst = std::slice::from_raw_parts_mut(uv.add(row * uv_stride), cw * 2);
                    for column in 0..cw {
                        let from = if self.mirrored {
                            cw - 1 - column
                        } else {
                            column
                        };
                        dst[2 * column] = CHROMA[u[from] as usize];
                        dst[2 * column + 1] = CHROMA[v[from] as usize];
                    }
                }
            }
            CVPixelBufferUnlockBaseAddress(target, 0);
        }
        Some(out)
    }
}

/// Copies one row of a plane, through a range table, flipped for a mirror.
fn copy_row(src: &[u8], dst: &mut [u8], bytes_per_pixel: usize, mirrored: bool, lut: &[u8; 256]) {
    if !mirrored {
        for (d, s) in dst.iter_mut().zip(src) {
            *d = lut[*s as usize];
        }
        return;
    }
    let pixels = src.len() / bytes_per_pixel;
    for p in 0..pixels {
        let from = (pixels - 1 - p) * bytes_per_pixel;
        for b in 0..bytes_per_pixel {
            dst[p * bytes_per_pixel + b] = lut[src[from + b] as usize];
        }
    }
}

const fn table(kind: u8) -> [u8; 256] {
    let mut out = [0u8; 256];
    let mut i = 0;
    while i < 256 {
        let v = i as i32;
        let mapped = match kind {
            0 => v,
            1 => (v - 16) * 255 / 219,
            _ => (v - 128) * 255 / 224 + 128,
        };
        out[i] = if mapped < 0 {
            0
        } else if mapped > 255 {
            255
        } else {
            mapped as u8
        };
        i += 1;
    }
    out
}
static IDENTITY: [u8; 256] = table(0);
/// Video range (16…235 luma, 16…240 chroma) to full range.
static LUMA: [u8; 256] = table(1);
static CHROMA: [u8; 256] = table(2);

fn make_pool(width: usize, height: usize) -> Option<CVPixelBufferPool> {
    let key = |k: core_foundation::string::CFStringRef| unsafe { CFString::wrap_under_get_rule(k) };
    let empty: CFDictionary<CFString, CFType> = CFDictionary::from_CFType_pairs(&[]);
    let attributes = CFDictionary::from_CFType_pairs(&[
        (
            key(unsafe { kCVPixelBufferPixelFormatTypeKey }),
            CFNumber::from(kCVPixelFormatType_420YpCbCr8BiPlanarFullRange as i64).as_CFType(),
        ),
        (
            key(unsafe { kCVPixelBufferWidthKey }),
            CFNumber::from(width as i64).as_CFType(),
        ),
        (
            key(unsafe { kCVPixelBufferHeightKey }),
            CFNumber::from(height as i64).as_CFType(),
        ),
        // Metal reads it without a copy only from an IOSurface.
        (
            key(unsafe { kCVPixelBufferIOSurfacePropertiesKey }),
            empty.as_CFType(),
        ),
    ]);
    CVPixelBufferPool::new(None, Some(&attributes)).ok()
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements; the ivars are thread-safe.
    #[unsafe(super(NSObject))]
    #[name = "OxideVideoSink"]
    #[ivars = Arc<FrameSlot>]
    pub struct VideoSink;

    impl VideoSink {
        #[unsafe(method(setSize:))]
        fn set_size(&self, _size: CGSize) {}

        #[unsafe(method(renderFrame:))]
        fn render_frame(&self, frame: *mut AnyObject) {
            if let Some(frame) = unsafe { frame.as_ref() } {
                self.ivars().receive(frame);
            }
        }
    }
);

impl VideoSink {
    fn new(slot: Arc<FrameSlot>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(slot);
        unsafe { msg_send![super(this), init] }
    }
}

/// A WebRTC video track drawn in the app: its renderer and latest frame. Removing it detaches the renderer.
pub struct VideoFeed {
    pub slot: Arc<FrameSlot>,
    pub track: Obj,
    sink: Retained<VideoSink>,
}

impl VideoFeed {
    pub fn attach(track: Obj, slot: Arc<FrameSlot>) -> std::rc::Rc<Self> {
        let sink = VideoSink::new(slot.clone());
        unsafe {
            let _: () = msg_send![&*track, addRenderer: &*sink];
        }
        std::rc::Rc::new(Self { slot, track, sink })
    }

    pub fn track_id(&self) -> String {
        let id: Option<Obj> = unsafe { msg_send![&*self.track, trackId] };
        crate::media_objc::string(id.as_deref())
    }
}

impl Drop for VideoFeed {
    fn drop(&mut self) {
        unsafe {
            let _: () = msg_send![&*self.track, removeRenderer: &*self.sink];
        }
    }
}

/// The video of `slot`, filling (`fit`: whole picture, letterboxed) a black box; nothing drawn before a first frame.
pub fn view(slot: &FrameSlot, fit: bool) -> AnyElement {
    div()
        .size_full()
        .overflow_hidden()
        .bg(gpui::black())
        .when_some(slot.take(), |d, frame| {
            d.child(
                surface(frame)
                    .object_fit(if fit {
                        ObjectFit::Contain
                    } else {
                        ObjectFit::Cover
                    })
                    .size_full(),
            )
        })
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_tables_expand_video_range() {
        assert_eq!(LUMA[16], 0);
        assert_eq!(LUMA[235], 255);
        assert_eq!(CHROMA[128], 128);
        assert!(CHROMA[16] <= 1);
        assert_eq!(IDENTITY[77], 77);
    }

    #[test]
    fn mirrored_rows_keep_chroma_pairs() {
        let mut dst = [0u8; 4];
        copy_row(&[1, 2, 3, 4], &mut dst, 2, true, &IDENTITY);
        assert_eq!(dst, [3, 4, 1, 2]);
        copy_row(&[1, 2, 3, 4], &mut dst, 1, true, &IDENTITY);
        assert_eq!(dst, [4, 3, 2, 1]);
    }

    #[test]
    fn pool_makes_full_range_nv12() {
        let pool = make_pool(64, 48).expect("pool");
        let buffer = pool.create_pixel_buffer().expect("buffer");
        assert_eq!(
            buffer.get_pixel_format(),
            kCVPixelFormatType_420YpCbCr8BiPlanarFullRange
        );
        assert_eq!((buffer.get_width(), buffer.get_height()), (64, 48));
    }
}

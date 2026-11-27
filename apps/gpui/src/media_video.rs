//! Video frames to the screen (`VideoLayerView.swift`): the engine's sink for one slot keeps only the latest frame,
//! drops the next ones until the window has drawn it, and converts on the engine's thread.
//!
//! macOS: a full-range NV12 `CVPixelBuffer` (the decoder's or camera's own when it already is one), drawn by GPUI's
//! `surface`. Elsewhere: BGRA in a `RenderImage`, painted as an image and taken out of the window's atlas once replaced.
use crate::media_engine::{MediaMsg, Window as Target};
use futures::channel::mpsc::UnboundedSender;
use gpui::{AnyElement, div, prelude::*};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use teams_media::{VideoFrame, VideoPlanes};

#[cfg(target_os = "macos")]
pub use mac::Picture;
#[cfg(not(target_os = "macos"))]
pub use other::Picture;

/// The latest frame of one video, shared by the engine's thread (or a camera preview's) and the window drawing it.
pub struct FrameSlot {
    latest: Mutex<Option<Picture>>,
    /// A frame waits for the window: newer ones are dropped until it is drawn, as the macOS app does.
    pending: AtomicBool,
    /// Our self-view is shown mirrored.
    mirrored: bool,
    /// Reused NV12 buffers, rebuilt when the resolution changes.
    #[cfg(target_os = "macos")]
    pool: Mutex<Option<mac::Pool>>,
    /// Images each window last painted, taken out of its atlas when the next one replaces them.
    #[cfg(not(target_os = "macos"))]
    shown: Mutex<Vec<(gpui::WindowId, Arc<gpui::RenderImage>)>>,
    wake: UnboundedSender<MediaMsg>,
    target: Target,
}

impl FrameSlot {
    pub fn new(mirrored: bool, target: Target, wake: UnboundedSender<MediaMsg>) -> Arc<Self> {
        Arc::new(Self {
            latest: Mutex::new(None),
            pending: AtomicBool::new(false),
            mirrored,
            #[cfg(target_os = "macos")]
            pool: Mutex::new(None),
            #[cfg(not(target_os = "macos"))]
            shown: Mutex::new(Vec::new()),
            wake,
            target,
        })
    }

    /// The frame to draw now; lets the next one in.
    pub fn take(&self) -> Option<Picture> {
        let frame = self.latest.lock().unwrap().clone();
        self.pending.store(false, Ordering::Release);
        frame
    }

    /// Claims the right to publish a frame; false while the last one waits for the window.
    fn claim(&self) -> bool {
        !self.pending.swap(true, Ordering::AcqRel)
    }

    fn publish(&self, picture: Option<Picture>) {
        match picture {
            Some(picture) => {
                *self.latest.lock().unwrap() = Some(picture);
                let _ = self.wake.unbounded_send(MediaMsg::Frame(self.target));
            }
            None => self.pending.store(false, Ordering::Release),
        }
    }

    /// A frame from the engine, on its thread: converted only when the window has taken the last one.
    pub fn receive(&self, frame: &VideoFrame<'_>) {
        if frame.width == 0 || frame.height == 0 || !self.claim() {
            return;
        }
        let picture = self.convert(frame);
        self.publish(picture);
    }
}

impl teams_media::VideoSink for FrameSlot {
    fn on_frame(&self, frame: &VideoFrame<'_>) {
        self.receive(frame);
    }
}

/// An engine video slot drawn in the app: our camera, or a remote line.
pub struct VideoFeed {
    pub slot: Arc<FrameSlot>,
    pub source: teams_media::VideoSlot,
}

// MARK: Pixels

/// 8-bit 4:2:0 planes, read through a rotation and a mirror.
struct Planes<'a> {
    width: usize,
    height: usize,
    y: &'a [u8],
    stride_y: usize,
    chroma: Chroma<'a>,
    /// Full range (0…255); otherwise video range (16…235 luma).
    full: bool,
}

enum Chroma<'a> {
    I420 {
        u: &'a [u8],
        v: &'a [u8],
        stride_u: usize,
        stride_v: usize,
    },
    Nv12 {
        uv: &'a [u8],
        stride_uv: usize,
    },
}

impl<'a> Planes<'a> {
    fn of(frame: &VideoFrame<'a>) -> Option<Self> {
        let (width, height) = (frame.width as usize, frame.height as usize);
        let (y, stride_y, chroma) = match &frame.planes {
            VideoPlanes::I420 {
                y,
                u,
                v,
                stride_y,
                stride_u,
                stride_v,
            } => (
                *y,
                *stride_y as usize,
                Chroma::I420 {
                    u,
                    v,
                    stride_u: *stride_u as usize,
                    stride_v: *stride_v as usize,
                },
            ),
            VideoPlanes::Nv12 {
                y,
                uv,
                stride_y,
                stride_uv,
            } => (
                *y,
                *stride_y as usize,
                Chroma::Nv12 {
                    uv,
                    stride_uv: *stride_uv as usize,
                },
            ),
            VideoPlanes::CvPixelBuffer(_) => return None,
        };
        let planes = Self {
            width,
            height,
            y,
            stride_y,
            chroma,
            full: false,
        };
        planes.fits().then_some(planes)
    }

    /// Every pixel read stays inside the slices.
    fn fits(&self) -> bool {
        let fits = |len: usize, stride: usize, rows: usize, bytes: usize| {
            stride >= bytes && len >= (rows - 1) * stride + bytes
        };
        if self.width == 0 || self.height == 0 {
            return false;
        }
        let (cw, ch) = (self.width.div_ceil(2), self.height.div_ceil(2));
        fits(self.y.len(), self.stride_y, self.height, self.width)
            && match &self.chroma {
                Chroma::I420 {
                    u,
                    v,
                    stride_u,
                    stride_v,
                } => fits(u.len(), *stride_u, ch, cw) && fits(v.len(), *stride_v, ch, cw),
                Chroma::Nv12 { uv, stride_uv } => fits(uv.len(), *stride_uv, ch, 2 * cw),
            }
    }

    fn luma(&self, x: usize, y: usize) -> u8 {
        self.y[y * self.stride_y + x]
    }

    /// Cb, Cr at a luma position.
    fn chroma(&self, x: usize, y: usize) -> (u8, u8) {
        let (x, y) = (x / 2, y / 2);
        match &self.chroma {
            Chroma::I420 {
                u,
                v,
                stride_u,
                stride_v,
            } => (u[y * stride_u + x], v[y * stride_v + x]),
            Chroma::Nv12 { uv, stride_uv } => {
                (uv[y * stride_uv + 2 * x], uv[y * stride_uv + 2 * x + 1])
            }
        }
    }
}

/// Where the pixels go: the picture's size once rotated, and which source pixel each one shows.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Geometry {
    source: (usize, usize),
    rotation: u16,
    mirrored: bool,
}

impl Geometry {
    fn size(&self) -> (usize, usize) {
        let (w, h) = self.source;
        if self.rotation % 180 == 90 {
            (h, w)
        } else {
            (w, h)
        }
    }

    /// The source pixel shown at `(x, y)`: rotated clockwise by `rotation`, then mirrored.
    fn source_of(&self, x: usize, y: usize) -> (usize, usize) {
        let (w, h) = self.source;
        let (out_w, _) = self.size();
        let x = if self.mirrored { out_w - 1 - x } else { x };
        match self.rotation {
            90 => (y, h - 1 - x),
            180 => (w - 1 - x, h - 1 - y),
            270 => (w - 1 - y, x),
            _ => (x, y),
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

/// Writes `planes` as full-range NV12 into `y_out` / `uv_out` (rows of `stride_y` / `stride_uv` bytes).
fn write_nv12(
    planes: &Planes<'_>,
    geometry: Geometry,
    y_out: &mut [u8],
    stride_y: usize,
    uv_out: &mut [u8],
    stride_uv: usize,
) {
    let (luma, chroma) = if planes.full {
        (&IDENTITY, &IDENTITY)
    } else {
        (&LUMA, &CHROMA)
    };
    let (w, h) = geometry.size();
    for y in 0..h {
        let row = &mut y_out[y * stride_y..y * stride_y + w];
        for (x, out) in row.iter_mut().enumerate() {
            let (sx, sy) = geometry.source_of(x, y);
            *out = luma[planes.luma(sx, sy) as usize];
        }
    }
    let (cw, ch) = (w.div_ceil(2), h.div_ceil(2));
    for y in 0..ch {
        let row = &mut uv_out[y * stride_uv..y * stride_uv + 2 * cw];
        for x in 0..cw {
            let (sx, sy) = geometry.source_of((2 * x).min(w - 1), (2 * y).min(h - 1));
            let (cb, cr) = planes.chroma(sx, sy);
            row[2 * x] = chroma[cb as usize];
            row[2 * x + 1] = chroma[cr as usize];
        }
    }
}

/// BT.601 to BGRA, premultiplied (opaque).
#[cfg_attr(target_os = "macos", allow(dead_code))]
fn write_bgra(planes: &Planes<'_>, geometry: Geometry, out: &mut [u8]) {
    let (w, h) = geometry.size();
    for y in 0..h {
        for x in 0..w {
            let (sx, sy) = geometry.source_of(x, y);
            let (cb, cr) = planes.chroma(sx, sy);
            let (l, d, e) = (
                i32::from(planes.luma(sx, sy)),
                i32::from(cb) - 128,
                i32::from(cr) - 128,
            );
            let (r, g, b) = if planes.full {
                (
                    l + ((359 * e) >> 8),
                    l - ((88 * d + 183 * e) >> 8),
                    l + ((454 * d) >> 8),
                )
            } else {
                let c = 298 * (l - 16);
                (
                    (c + 409 * e + 128) >> 8,
                    (c - 100 * d - 208 * e + 128) >> 8,
                    (c + 516 * d + 128) >> 8,
                )
            };
            let at = (y * w + x) * 4;
            out[at] = b.clamp(0, 255) as u8;
            out[at + 1] = g.clamp(0, 255) as u8;
            out[at + 2] = r.clamp(0, 255) as u8;
            out[at + 3] = 255;
        }
    }
}

// MARK: macOS

#[cfg(target_os = "macos")]
mod mac {
    use super::*;
    use core_foundation::base::{CFType, TCFType};
    use core_foundation::dictionary::CFDictionary;
    use core_foundation::number::CFNumber;
    use core_foundation::string::CFString;
    use core_video::pixel_buffer::{
        CVPixelBuffer, CVPixelBufferGetBaseAddressOfPlane, CVPixelBufferGetBytesPerRowOfPlane,
        CVPixelBufferGetHeightOfPlane, CVPixelBufferLockBaseAddress, CVPixelBufferRef,
        CVPixelBufferUnlockBaseAddress, kCVPixelBufferHeightKey,
        kCVPixelBufferIOSurfacePropertiesKey, kCVPixelBufferPixelFormatTypeKey,
        kCVPixelBufferWidthKey, kCVPixelFormatType_420YpCbCr8BiPlanarFullRange,
        kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
    };
    use core_video::pixel_buffer_pool::CVPixelBufferPool;

    /// A pixel buffer handed between the engine's threads and the UI (CoreVideo buffers are thread-safe).
    #[derive(Clone)]
    pub struct Picture(pub CVPixelBuffer);
    unsafe impl Send for Picture {}
    unsafe impl Sync for Picture {}

    pub struct Pool {
        pool: CVPixelBufferPool,
        width: usize,
        height: usize,
    }
    unsafe impl Send for Pool {}

    const READ_ONLY: u64 = 1;

    impl FrameSlot {
        pub(super) fn convert(&self, frame: &VideoFrame<'_>) -> Option<Picture> {
            let geometry = Geometry {
                source: (frame.width as usize, frame.height as usize),
                rotation: frame.rotation,
                mirrored: self.mirrored,
            };
            match frame.planes {
                VideoPlanes::CvPixelBuffer(pixels) => {
                    self.convert_pixel_buffer(pixels as CVPixelBufferRef, geometry)
                }
                _ => {
                    let planes = Planes::of(frame)?;
                    self.copy(&planes, geometry)
                }
            }
        }

        /// A camera preview's frame (`CMSampleBuffer`'s image), on the capture queue.
        pub fn receive_pixels(&self, pixels: CVPixelBufferRef) {
            if pixels.is_null() || !self.claim() {
                return;
            }
            let geometry = Geometry {
                source: (0, 0),
                rotation: 0,
                mirrored: self.mirrored,
            };
            let picture = self.convert_pixel_buffer(pixels, geometry);
            self.publish(picture);
        }

        /// The decoder's or camera's buffer: kept as is when already what `surface` draws, else copied.
        fn convert_pixel_buffer(
            &self,
            pixels: CVPixelBufferRef,
            geometry: Geometry,
        ) -> Option<Picture> {
            if pixels.is_null() {
                return None;
            }
            // Retained: the engine's reference only lasts the callback.
            let pixels = unsafe { CVPixelBuffer::wrap_under_get_rule(pixels) };
            let format = pixels.get_pixel_format();
            let full = format == kCVPixelFormatType_420YpCbCr8BiPlanarFullRange;
            if !full && format != kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange {
                return None;
            }
            let geometry = Geometry {
                source: (pixels.get_width(), pixels.get_height()),
                ..geometry
            };
            if full && geometry.rotation == 0 && !geometry.mirrored {
                return Some(Picture(pixels));
            }
            let source = pixels.as_concrete_TypeRef();
            unsafe {
                if CVPixelBufferLockBaseAddress(source, READ_ONLY) != 0 {
                    return None;
                }
                let plane = |index: usize| {
                    let base = CVPixelBufferGetBaseAddressOfPlane(source, index) as *const u8;
                    let stride = CVPixelBufferGetBytesPerRowOfPlane(source, index);
                    let rows = CVPixelBufferGetHeightOfPlane(source, index);
                    (!base.is_null())
                        .then(|| (std::slice::from_raw_parts(base, stride * rows), stride))
                };
                let copied = match (plane(0), plane(1)) {
                    (Some((y, stride_y)), Some((uv, stride_uv))) => {
                        let (width, height) = geometry.source;
                        let planes = Planes {
                            width,
                            height,
                            y,
                            stride_y,
                            chroma: Chroma::Nv12 { uv, stride_uv },
                            full,
                        };
                        planes
                            .fits()
                            .then(|| self.copy(&planes, geometry))
                            .flatten()
                    }
                    _ => None,
                };
                CVPixelBufferUnlockBaseAddress(source, READ_ONLY);
                copied
            }
        }

        /// Into a pooled full-range NV12 buffer.
        fn copy(&self, planes: &Planes<'_>, geometry: Geometry) -> Option<Picture> {
            let (width, height) = geometry.size();
            let out = self.buffer(width, height)?;
            let target = out.as_concrete_TypeRef();
            unsafe {
                if CVPixelBufferLockBaseAddress(target, 0) != 0 {
                    return None;
                }
                let y = CVPixelBufferGetBaseAddressOfPlane(target, 0) as *mut u8;
                let uv = CVPixelBufferGetBaseAddressOfPlane(target, 1) as *mut u8;
                let stride_y = CVPixelBufferGetBytesPerRowOfPlane(target, 0);
                let stride_uv = CVPixelBufferGetBytesPerRowOfPlane(target, 1);
                let rows_uv = CVPixelBufferGetHeightOfPlane(target, 1);
                let ok = !y.is_null() && !uv.is_null();
                if ok {
                    write_nv12(
                        planes,
                        geometry,
                        std::slice::from_raw_parts_mut(y, stride_y * height),
                        stride_y,
                        std::slice::from_raw_parts_mut(uv, stride_uv * rows_uv),
                        stride_uv,
                    );
                }
                CVPixelBufferUnlockBaseAddress(target, 0);
                ok.then_some(Picture(out))
            }
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
    }

    pub(super) fn make_pool(width: usize, height: usize) -> Option<CVPixelBufferPool> {
        let key =
            |k: core_foundation::string::CFStringRef| unsafe { CFString::wrap_under_get_rule(k) };
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

    /// The picture, filling (`fit`: whole, letterboxed) the box.
    pub(super) fn picture(picture: Picture, fit: bool) -> AnyElement {
        gpui::surface(picture.0)
            .object_fit(if fit {
                gpui::ObjectFit::Contain
            } else {
                gpui::ObjectFit::Cover
            })
            .size_full()
            .into_any_element()
    }
}

// MARK: Windows and Linux

#[cfg(not(target_os = "macos"))]
mod other {
    use super::*;
    use gpui::{Bounds, Corners, RenderImage, Window, canvas, point, px, size};

    pub type Picture = Arc<RenderImage>;

    impl FrameSlot {
        pub(super) fn convert(&self, frame: &VideoFrame<'_>) -> Option<Picture> {
            let planes = Planes::of(frame)?;
            let geometry = Geometry {
                source: (planes.width, planes.height),
                rotation: frame.rotation,
                mirrored: self.mirrored,
            };
            let (width, height) = geometry.size();
            let mut pixels = vec![0u8; width * height * 4];
            write_bgra(&planes, geometry, &mut pixels);
            let image = image::RgbaImage::from_raw(width as u32, height as u32, pixels)?;
            Some(Arc::new(RenderImage::new(vec![image::Frame::new(image)])))
        }

        /// Notes that `window` now shows `image`, and takes the one it showed before out of its atlas.
        fn shown_in(&self, image: &Picture, window: &mut Window) {
            let id = window.window_handle().window_id();
            let mut shown = self.shown.lock().unwrap();
            match shown.iter_mut().find(|(w, _)| *w == id) {
                Some((_, last)) if Arc::ptr_eq(last, image) => {}
                Some((_, last)) => {
                    let old = std::mem::replace(last, image.clone());
                    let _ = window.drop_image(old);
                }
                None => shown.push((id, image.clone())),
            }
        }
    }

    impl Drop for FrameSlot {
        fn drop(&mut self) {
            // The windows free them the next time they draw (`sweep`).
            for (window, image) in std::mem::take(&mut *self.shown.lock().unwrap()) {
                retire(window, image);
            }
        }
    }

    /// The picture, filling (`fit`: whole, letterboxed) the box.
    pub(super) fn picture(slot: Arc<FrameSlot>, picture: Picture, fit: bool) -> AnyElement {
        canvas(
            |_, _, _| {},
            move |bounds, _, window, _| {
                slot.shown_in(&picture, window);
                let source = picture.size(0);
                let (iw, ih) = (source.width.0 as f32, source.height.0 as f32);
                let (bw, bh) = (f32::from(bounds.size.width), f32::from(bounds.size.height));
                if iw <= 0. || ih <= 0. || bw <= 0. || bh <= 0. {
                    return;
                }
                let scale = if fit {
                    (bw / iw).min(bh / ih)
                } else {
                    (bw / iw).max(bh / ih)
                };
                let (w, h) = (iw * scale, ih * scale);
                let target = Bounds::new(
                    bounds.origin + point(px((bw - w) / 2.), px((bh - h) / 2.)),
                    size(px(w), px(h)),
                );
                let _ = window.paint_image(target, Corners::default(), picture.clone(), 0, false);
            },
        )
        .size_full()
        .into_any_element()
    }
}

/// Images no longer drawn but still in a window's atlas: video frames of slots gone since, share sheet thumbnails.
static RETIRED: Mutex<Vec<(gpui::WindowId, Arc<gpui::RenderImage>)>> = Mutex::new(Vec::new());

/// Frees `image` from `window`'s atlas the next time that window draws.
pub fn retire(window: gpui::WindowId, image: Arc<gpui::RenderImage>) {
    RETIRED.lock().unwrap().push((window, image));
}

/// Frees the images retired from this window; called as each window draws.
pub fn sweep(window: &mut gpui::Window) {
    let id = window.window_handle().window_id();
    let mut retired = RETIRED.lock().unwrap();
    if retired.is_empty() {
        return;
    }
    for (_, image) in retired.extract_if(.., |(w, _)| *w == id) {
        let _ = window.drop_image(image);
    }
}

/// The video of `slot`, filling (`fit`: whole picture, letterboxed) a black box; nothing drawn before a first frame.
pub fn view(slot: &Arc<FrameSlot>, fit: bool) -> AnyElement {
    let picture = slot.take();
    div()
        .size_full()
        .overflow_hidden()
        .bg(gpui::black())
        .when_some(picture, |d, picture| {
            #[cfg(target_os = "macos")]
            let element = mac::picture(picture, fit);
            #[cfg(not(target_os = "macos"))]
            let element = other::picture(slot.clone(), picture, fit);
            d.child(element)
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

    fn geometry(rotation: u16, mirrored: bool) -> Geometry {
        Geometry {
            source: (4, 2),
            rotation,
            mirrored,
        }
    }

    #[test]
    fn rotations_swap_the_size_and_map_corners() {
        assert_eq!(geometry(0, false).size(), (4, 2));
        assert_eq!(geometry(90, false).size(), (2, 4));
        // Clockwise: the source's bottom-left corner ends up top-left.
        assert_eq!(geometry(90, false).source_of(0, 0), (0, 1));
        assert_eq!(geometry(90, false).source_of(1, 3), (3, 0));
        assert_eq!(geometry(180, false).source_of(0, 0), (3, 1));
        assert_eq!(geometry(270, false).source_of(0, 0), (3, 0));
        assert_eq!(geometry(0, true).source_of(0, 0), (3, 0));
    }

    #[test]
    fn nv12_and_bgra_from_i420() {
        // 2×2 mid grey in video range.
        let (y, u, v) = ([126u8; 4], [128u8; 1], [128u8; 1]);
        let frame = VideoFrame {
            width: 2,
            height: 2,
            rotation: 0,
            planes: VideoPlanes::I420 {
                y: &y,
                u: &u,
                v: &v,
                stride_y: 2,
                stride_u: 1,
                stride_v: 1,
            },
        };
        let planes = Planes::of(&frame).expect("planes");
        let g = Geometry {
            source: (2, 2),
            rotation: 0,
            mirrored: false,
        };
        let (mut out_y, mut out_uv) = ([0u8; 4], [0u8; 2]);
        write_nv12(&planes, g, &mut out_y, 2, &mut out_uv, 2);
        assert_eq!(out_y, [LUMA[126]; 4]);
        assert_eq!(out_uv, [128, 128]);
        let mut bgra = [0u8; 16];
        write_bgra(&planes, g, &mut bgra);
        assert_eq!(&bgra[..4], &[128, 128, 128, 255]);
    }

    #[test]
    fn short_planes_are_refused() {
        let (y, u, v) = ([0u8; 3], [0u8; 1], [0u8; 1]);
        let frame = VideoFrame {
            width: 2,
            height: 2,
            rotation: 0,
            planes: VideoPlanes::I420 {
                y: &y,
                u: &u,
                v: &v,
                stride_y: 2,
                stride_u: 1,
                stride_v: 1,
            },
        };
        assert!(Planes::of(&frame).is_none());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn pool_makes_full_range_nv12() {
        use core_video::pixel_buffer::kCVPixelFormatType_420YpCbCr8BiPlanarFullRange;
        let pool = mac::make_pool(64, 48).expect("pool");
        let buffer = pool.create_pixel_buffer().expect("buffer");
        assert_eq!(
            buffer.get_pixel_format(),
            kCVPixelFormatType_420YpCbCr8BiPlanarFullRange
        );
        assert_eq!((buffer.get_width(), buffer.get_height()), (64, 48));
    }
}

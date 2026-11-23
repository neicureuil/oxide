//! Teams fork: the macOS camera through libwebrtc's `RTCCameraVideoCapturer`, feeding a video source.
use cxx::UniquePtr;
use webrtc_sys::camera_mac::ffi;

use crate::video_source::native::NativeVideoSource;

/// A capturing camera; dropping it stops the capture.
pub struct MacCamera {
    _handle: UniquePtr<ffi::MacCamera>,
}

impl MacCamera {
    /// The camera with this AVFoundation `uniqueID`, else `name`, else the default one; 720p, ≤ 30 fps.
    pub fn start(source: &NativeVideoSource, unique_id: &str, name: &str) -> Option<Self> {
        let handle =
            ffi::start_mac_camera(source.handle.sys_handle(), unique_id.into(), name.into());
        (!handle.is_null()).then_some(Self { _handle: handle })
    }
}

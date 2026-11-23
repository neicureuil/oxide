//! Teams fork: the macOS camera through libwebrtc's `RTCCameraVideoCapturer`, feeding a video source.
use cxx::UniquePtr;
use webrtc_sys::camera_mac::ffi;

use crate::video_source::native::NativeVideoSource;

/// A capturing camera; dropping it stops the capture.
pub struct MacCamera {
    _handle: UniquePtr<ffi::MacCamera>,
}

impl MacCamera {
    /// The camera with this AVFoundation `uniqueID`, else this name, else the default one; 720p when
    /// available, at most 30 fps. `None` without a camera.
    pub fn start(source: &NativeVideoSource, unique_id: &str, name: &str) -> Option<Self> {
        let handle = ffi::start_mac_camera(source.handle.sys_handle(), unique_id.into(), name.into());
        (!handle.is_null()).then_some(Self { _handle: handle })
    }

    /// Frames AVFoundation delivered so far.
    pub fn frames(&self) -> u64 {
        self._handle.frames()
    }
}

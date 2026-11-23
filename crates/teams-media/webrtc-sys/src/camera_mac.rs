//! Teams fork: the macOS camera through libwebrtc's `RTCCameraVideoCapturer`.

#[cxx::bridge(namespace = "livekit_ffi")]
pub mod ffi {
    unsafe extern "C++" {
        include!("livekit/camera_mac.h");

        type MacCamera;
        type VideoTrackSource = crate::video_track::ffi::VideoTrackSource;

        fn start_mac_camera(
            source: SharedPtr<VideoTrackSource>,
            unique_id: String,
            name: String,
        ) -> UniquePtr<MacCamera>;
        fn stop(self: &MacCamera);
        fn frames(self: &MacCamera) -> u64;
    }
}

// The capturer works from any thread; stopping it waits for AVFoundation.
unsafe impl Send for ffi::MacCamera {}
unsafe impl Sync for ffi::MacCamera {}

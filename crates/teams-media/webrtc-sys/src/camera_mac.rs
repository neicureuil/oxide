//! Teams fork: the macOS camera through libwebrtc's `RTCCameraVideoCapturer`.

/// Runs once when the capture can't start or stops on its own.
pub struct CameraFailure(pub Box<dyn Fn() + Send + Sync>);

fn camera_failed(failure: &CameraFailure) {
    (failure.0)()
}

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
            failure: Box<CameraFailure>,
        ) -> UniquePtr<MacCamera>;
    }

    extern "Rust" {
        type CameraFailure;
        fn camera_failed(failure: &CameraFailure);
    }
}

// The capturer works from any thread; stopping it waits for AVFoundation.
unsafe impl Send for ffi::MacCamera {}
unsafe impl Sync for ffi::MacCamera {}

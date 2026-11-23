//! A Teams call's media on libwebrtc, in standard SDP (`teams_calling::sdp` converts to Teams' NGC).

mod audio;
mod camera;
mod devices;
mod engine;
mod screen;
mod stats;
mod video;

use std::collections::BTreeSet;
use std::sync::Arc;

pub use camera::{CameraDevice, cameras};
pub use devices::{AudioDevice, DeviceChoice, audio_devices};
pub use engine::MediaEngine;
pub use screen::{ScreenSource, ScreenSourceKind, screen_sources};
pub use stats::CallDiagnostics;
pub use video::{VideoFrame, VideoPlanes, VideoSink, VideoSlot};

/// A TURN or STUN server, as TRAP gives them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IceServer {
    pub urls: Vec<String>,
    pub username: Option<String>,
    pub credential: Option<String>,
}

/// Opens the call's peer connection (`Effect::MediaPrepare`).
#[derive(Debug, Clone)]
pub struct PrepareOptions {
    pub call_id: String,
    pub ice_servers: Vec<IceServer>,
    /// Send the camera from the start.
    pub video: bool,
    /// Start with the microphone off.
    pub muted: bool,
    /// Play the other participants on this device.
    pub speaker: bool,
    /// Video lines for cameras (ours included) when offering; the sharing line comes on top.
    pub camera_lines: u32,
    /// Answer this offer instead of making one (an incoming call).
    pub remote_offer: Option<String>,
}

/// The mixer's bounds for one of our sending lines (`controlVideoStreaming` `fmtParams`).
#[derive(Debug, Clone, PartialEq)]
pub struct SendLimits {
    /// WebRTC mid of the line.
    pub mid: String,
    pub sharing: bool,
    /// Macroblocks (16×16) per frame.
    pub max_fs: u32,
    pub max_fps: f64,
    /// Bits per second; 0 keeps the encoder's own.
    pub max_bitrate: u32,
    pub keyframe: bool,
}

/// Devices the call uses; `None` is the system default.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Devices {
    pub microphone: Option<DeviceChoice>,
    pub speaker: Option<DeviceChoice>,
    pub camera: Option<DeviceChoice>,
}

/// What the engine reports. The first six are the core's `CallAction`s; the others are for the call window.
#[derive(Debug, Clone, PartialEq)]
pub enum MediaEvent {
    /// Our offer or answer, ICE gathering complete (NGC takes no trickled candidates).
    LocalDescription {
        call_id: String,
        sdp: String,
    },
    /// Our answer to [`MediaEngine::answer_remote_offer`].
    RenegotiationAnswer {
        call_id: String,
        sdp: String,
    },
    Connected {
        call_id: String,
    },
    /// The call's media is gone; the engine has closed it.
    Failed {
        call_id: String,
        reason: String,
    },
    /// Screen sharing ended without the user asking (refused, capture stopped).
    ScreenShareStopped {
        call_id: String,
        reason: Option<String>,
    },
    /// The camera stopped without the user asking (the service refused it).
    CameraStopped {
        call_id: String,
        reason: Option<String>,
    },
    /// CSRCs heard in the mixed audio just now: who is speaking.
    Speaking {
        call_id: String,
        sources: BTreeSet<u32>,
    },
    /// Our microphone picks up a voice.
    LocalSpeaking {
        call_id: String,
        speaking: bool,
    },
    /// The media connection dropped and is being restored.
    Reconnecting {
        call_id: String,
        reconnecting: bool,
    },
    /// A video line started or stopped carrying something to show.
    Video {
        call_id: String,
        slot: VideoSlot,
        present: bool,
    },
    /// What the engine saw so far (every 2 s while connected, and on failure).
    Diagnostics {
        call_id: String,
        diagnostics: CallDiagnostics,
    },
}

/// Receives the engine's events, on one of its threads.
pub trait MediaListener: Send + Sync {
    fn on_event(&self, event: MediaEvent);
}

impl<F: Fn(MediaEvent) + Send + Sync> MediaListener for F {
    fn on_event(&self, event: MediaEvent) {
        self(event)
    }
}

/// Raw PCM taps for a local recording: 16-bit interleaved samples.
pub trait PcmSink: Send + Sync {
    /// `remote` is false for our microphone after echo cancellation and noise suppression.
    fn on_pcm(&self, remote: bool, samples: &[i16], sample_rate: u32, channels: u32);
}

pub(crate) type Listener = Arc<dyn MediaListener>;

/// Failure texts from the shared catalog, as the shells show them.
pub(crate) fn text(key: &str) -> String {
    teams_i18n::t(&format!("ui.media_engine.{key}")).to_string()
}

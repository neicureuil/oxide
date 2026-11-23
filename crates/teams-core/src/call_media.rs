//! `teams-media` for the shells; video frames skip UniFFI through `teams_media_set_video_sink` (no copy).
use std::ffi::{CStr, c_char, c_void};
use std::sync::{Arc, Mutex, Weak};

use crate::app::call::IceServerView;
use teams_media as media;

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Enum)]
pub enum VideoSlotView {
    Camera,
    Screen,
    Remote { mid: String },
}

impl From<media::VideoSlot> for VideoSlotView {
    fn from(slot: media::VideoSlot) -> Self {
        match slot {
            media::VideoSlot::Camera => Self::Camera,
            media::VideoSlot::Screen => Self::Screen,
            media::VideoSlot::Remote(mid) => Self::Remote { mid },
        }
    }
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct CallDiagnosticsView {
    pub camera_requested: bool,
    pub camera_failed: bool,
    pub connected: bool,
    pub disconnects: u32,
    pub failed: bool,
    pub failure_reason: Option<String>,
    pub mic_energy: f64,
    pub audio_packets_received: u64,
    pub audio_packets_lost: u64,
    pub round_trip: f64,
    pub video_frames_sent: u64,
}

impl From<media::CallDiagnostics> for CallDiagnosticsView {
    fn from(d: media::CallDiagnostics) -> Self {
        Self {
            camera_requested: d.camera_requested,
            camera_failed: d.camera_failed,
            connected: d.connected,
            disconnects: d.disconnects,
            failed: d.failed,
            failure_reason: d.failure_reason,
            mic_energy: d.mic_energy,
            audio_packets_received: d.audio_packets_received,
            audio_packets_lost: d.audio_packets_lost,
            round_trip: d.round_trip,
            video_frames_sent: d.video_frames_sent,
        }
    }
}

/// `teams_media::MediaEvent`; the first six map onto `CallAction`s.
#[derive(Debug, Clone, PartialEq, uniffi::Enum)]
pub enum MediaEventView {
    LocalDescription {
        call_id: String,
        sdp: String,
    },
    RenegotiationAnswer {
        call_id: String,
        sdp: String,
    },
    Connected {
        call_id: String,
    },
    Failed {
        call_id: String,
        reason: String,
    },
    ScreenShareStopped {
        call_id: String,
        reason: Option<String>,
    },
    CameraStopped {
        call_id: String,
        reason: Option<String>,
    },
    Speaking {
        call_id: String,
        sources: Vec<u32>,
    },
    LocalSpeaking {
        call_id: String,
        speaking: bool,
    },
    Reconnecting {
        call_id: String,
        reconnecting: bool,
    },
    Video {
        call_id: String,
        slot: VideoSlotView,
        present: bool,
    },
    Diagnostics {
        call_id: String,
        diagnostics: CallDiagnosticsView,
    },
}

impl From<media::MediaEvent> for MediaEventView {
    fn from(event: media::MediaEvent) -> Self {
        use media::MediaEvent as E;
        match event {
            E::LocalDescription { call_id, sdp } => Self::LocalDescription { call_id, sdp },
            E::RenegotiationAnswer { call_id, sdp } => Self::RenegotiationAnswer { call_id, sdp },
            E::Connected { call_id } => Self::Connected { call_id },
            E::Failed { call_id, reason } => Self::Failed { call_id, reason },
            E::ScreenShareStopped { call_id, reason } => {
                Self::ScreenShareStopped { call_id, reason }
            }
            E::CameraStopped { call_id, reason } => Self::CameraStopped { call_id, reason },
            E::Speaking { call_id, sources } => Self::Speaking {
                call_id,
                sources: sources.into_iter().collect(),
            },
            E::LocalSpeaking { call_id, speaking } => Self::LocalSpeaking { call_id, speaking },
            E::Reconnecting {
                call_id,
                reconnecting,
            } => Self::Reconnecting {
                call_id,
                reconnecting,
            },
            E::Video {
                call_id,
                slot,
                present,
            } => Self::Video {
                call_id,
                slot: slot.into(),
                present,
            },
            E::Diagnostics {
                call_id,
                diagnostics,
            } => Self::Diagnostics {
                call_id,
                diagnostics: diagnostics.into(),
            },
        }
    }
}

/// Implemented by the shell. Called on media threads.
#[uniffi::export(with_foreign)]
pub trait MediaListener: Send + Sync {
    fn on_event(&self, event: MediaEventView);
}

/// Raw PCM for a local recording: 16-bit interleaved, `remote` false for our processed microphone.
#[uniffi::export(with_foreign)]
pub trait PcmListener: Send + Sync {
    fn on_pcm(&self, remote: bool, samples: Vec<i16>, sample_rate: u32, channels: u32);
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct DeviceChoiceView {
    pub id: String,
    pub name: String,
}

impl From<DeviceChoiceView> for media::DeviceChoice {
    fn from(c: DeviceChoiceView) -> Self {
        Self {
            id: c.id,
            name: c.name,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct AudioDeviceView {
    pub id: String,
    pub name: String,
    pub input: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ScreenSourceKindView {
    Screen,
    Window,
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ScreenSourceView {
    pub kind: ScreenSourceKindView,
    pub id: u64,
    pub title: String,
}

impl From<ScreenSourceKindView> for media::ScreenSourceKind {
    fn from(kind: ScreenSourceKindView) -> Self {
        match kind {
            ScreenSourceKindView::Screen => Self::Screen,
            ScreenSourceKindView::Window => Self::Window,
        }
    }
}

impl From<ScreenSourceView> for media::ScreenSource {
    fn from(s: ScreenSourceView) -> Self {
        Self {
            kind: s.kind.into(),
            id: s.id,
            title: s.title,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct ThumbnailView {
    pub width: u32,
    pub height: u32,
    /// RGBA rows.
    pub rgba: Vec<u8>,
}

/// The engine's progress lines go to `calling.log`, like the rest of the call.
struct MediaLog;

impl log::Log for MediaLog {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.target().starts_with("teams_media") && metadata.level() <= log::Level::Info
    }

    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) {
            teams_calling::diagnostics::record(&format!("media {}", record.args()));
        }
    }

    fn flush(&self) {}
}

fn install_media_log() {
    static LOG: MediaLog = MediaLog;
    if log::set_logger(&LOG).is_ok() {
        log::set_max_level(log::LevelFilter::Info);
    }
}

/// The engine the C video sink function acts on: one call engine per process.
static CURRENT: Mutex<Option<Weak<CallMedia>>> = Mutex::new(None);

#[derive(uniffi::Object)]
pub struct CallMedia {
    engine: media::MediaEngine,
}

#[uniffi::export]
impl CallMedia {
    /// `synthetic`: no device (a tone, a test pattern), for tests.
    #[uniffi::constructor]
    pub fn new(listener: Arc<dyn MediaListener>, synthetic: bool) -> Arc<Self> {
        install_media_log();
        let forward: Arc<dyn media::MediaListener> =
            Arc::new(move |event: media::MediaEvent| listener.on_event(event.into()));
        let engine = if synthetic {
            media::MediaEngine::synthetic(forward)
        } else {
            media::MediaEngine::new(forward)
        };
        let this = Arc::new(Self { engine });
        *CURRENT.lock().unwrap() = Some(Arc::downgrade(&this));
        this
    }

    #[allow(clippy::too_many_arguments)]
    pub fn prepare(
        &self,
        call_id: String,
        ice_servers: Vec<IceServerView>,
        video: bool,
        muted: bool,
        speaker: bool,
        camera_lines: u32,
        remote_offer: Option<String>,
    ) {
        let ice_servers = ice_servers
            .into_iter()
            .map(|s| media::IceServer {
                urls: s.urls,
                username: s.username,
                credential: s.credential,
            })
            .collect();
        self.engine.prepare(media::PrepareOptions {
            call_id,
            ice_servers,
            video,
            muted,
            speaker,
            camera_lines,
            remote_offer,
        });
    }

    pub fn set_remote_description(&self, call_id: String, sdp: String, provisional: bool) {
        self.engine
            .set_remote_description(&call_id, &sdp, provisional);
    }

    pub fn answer_remote_offer(&self, call_id: String, sdp: String) {
        self.engine.answer_remote_offer(&call_id, &sdp);
    }

    pub fn offer_rejected(&self, call_id: String, retry: bool) {
        self.engine.offer_rejected(&call_id, retry);
    }

    pub fn restart_ice(&self, call_id: String) {
        self.engine.restart_ice(&call_id);
    }

    pub fn set_muted(&self, call_id: String, muted: bool) {
        self.engine.set_muted(&call_id, muted);
    }

    pub fn set_speaker(&self, call_id: String, on: bool) {
        self.engine.set_speaker(&call_id, on);
    }

    pub fn set_camera(&self, call_id: String, on: bool) {
        self.engine.set_camera(&call_id, on);
    }

    pub fn set_screen_share(&self, call_id: String, on: bool, source: Option<ScreenSourceView>) {
        self.engine
            .set_screen_share(&call_id, on, source.map(Into::into));
    }

    #[allow(clippy::too_many_arguments)]
    pub fn apply_send_limits(
        &self,
        call_id: String,
        mid: String,
        sharing: bool,
        max_fs: u32,
        max_fps: f64,
        max_bitrate: u32,
        keyframe: bool,
    ) {
        let limits = media::SendLimits {
            mid,
            sharing,
            max_fs,
            max_fps,
            max_bitrate,
            keyframe,
        };
        self.engine.apply_send_limits(&call_id, limits);
    }

    pub fn set_devices(
        &self,
        microphone: Option<DeviceChoiceView>,
        speaker: Option<DeviceChoiceView>,
        camera: Option<DeviceChoiceView>,
    ) {
        self.engine.set_devices(media::Devices {
            microphone: microphone.map(Into::into),
            speaker: speaker.map(Into::into),
            camera: camera.map(Into::into),
        });
    }

    pub fn set_pcm_listener(&self, listener: Option<Arc<dyn PcmListener>>) {
        let sink = listener.map(|listener| Arc::new(Pcm(listener)) as Arc<dyn media::PcmSink>);
        self.engine.set_pcm_sink(sink);
    }

    pub fn close(&self, call_id: String) {
        self.engine.close(&call_id);
    }
}

struct Pcm(Arc<dyn PcmListener>);

impl media::PcmSink for Pcm {
    fn on_pcm(&self, remote: bool, samples: &[i16], sample_rate: u32, channels: u32) {
        self.0
            .on_pcm(remote, samples.to_vec(), sample_rate, channels);
    }
}

/// Microphones and speakers of the platform audio device. Opens it briefly.
#[uniffi::export]
pub fn media_audio_devices() -> Vec<AudioDeviceView> {
    media::audio_devices()
        .into_iter()
        .map(|d| AudioDeviceView {
            id: d.id,
            name: d.name,
            input: d.input,
        })
        .collect()
}

/// Cameras, with the ids `set_devices` matches.
#[uniffi::export]
pub fn media_cameras() -> Vec<DeviceChoiceView> {
    media::cameras()
        .into_iter()
        .map(|c| DeviceChoiceView {
            id: c.id,
            name: c.name,
        })
        .collect()
}

/// Screens or windows that can be shared now.
#[uniffi::export]
pub fn media_screen_sources(kind: ScreenSourceKindView) -> Vec<ScreenSourceView> {
    media::screen_sources(kind.into())
        .into_iter()
        .map(|s| ScreenSourceView {
            kind,
            id: s.id,
            title: s.title,
        })
        .collect()
}

/// One frame of `source`, at most `max_width` wide, for the share picker. Blocks up to 2 s.
#[uniffi::export]
pub fn media_screen_thumbnail(source: ScreenSourceView, max_width: u32) -> Option<ThumbnailView> {
    let source: media::ScreenSource = source.into();
    source
        .thumbnail(max_width)
        .map(|(width, height, rgba)| ThumbnailView {
            width,
            height,
            rgba,
        })
}

/// One frame for `TeamsVideoCallback`; pointers valid only during the call.
#[repr(C)]
pub struct TeamsVideoFrame {
    pub width: u32,
    pub height: u32,
    /// Clockwise degrees: 0, 90, 180, 270.
    pub rotation: u32,
    /// 0: I420 (planes 0-2), 1: NV12 (planes 0-1), 2: `pixel_buffer` (a CVPixelBufferRef).
    pub format: u32,
    pub planes: [*const u8; 3],
    pub strides: [u32; 3],
    pub pixel_buffer: *mut c_void,
}

pub type TeamsVideoCallback = extern "C" fn(context: *mut c_void, frame: *const TeamsVideoFrame);
pub type TeamsVideoRelease = extern "C" fn(context: *mut c_void);

struct CSink {
    context: *mut c_void,
    callback: TeamsVideoCallback,
    release: Option<TeamsVideoRelease>,
}

// The shell's context is made for any thread (a retained Swift object).
unsafe impl Send for CSink {}
unsafe impl Sync for CSink {}

impl Drop for CSink {
    fn drop(&mut self) {
        if let Some(release) = self.release {
            release(self.context);
        }
    }
}

impl media::VideoSink for CSink {
    fn on_frame(&self, frame: &media::VideoFrame<'_>) {
        let mut out = TeamsVideoFrame {
            width: frame.width,
            height: frame.height,
            rotation: u32::from(frame.rotation),
            format: 0,
            planes: [std::ptr::null(); 3],
            strides: [0; 3],
            pixel_buffer: std::ptr::null_mut(),
        };
        match &frame.planes {
            media::VideoPlanes::I420 {
                y,
                u,
                v,
                stride_y,
                stride_u,
                stride_v,
            } => {
                out.planes = [y.as_ptr(), u.as_ptr(), v.as_ptr()];
                out.strides = [*stride_y, *stride_u, *stride_v];
            }
            media::VideoPlanes::Nv12 {
                y,
                uv,
                stride_y,
                stride_uv,
            } => {
                out.format = 1;
                out.planes = [y.as_ptr(), uv.as_ptr(), std::ptr::null()];
                out.strides = [*stride_y, *stride_uv, 0];
            }
            media::VideoPlanes::CvPixelBuffer(pixels) => {
                out.format = 2;
                out.pixel_buffer = *pixels;
            }
        }
        (self.callback)(self.context, &out);
    }
}

/// Shows `slot` (0 camera, 1 screen, 2 remote `mid`) through `callback`, or stops when it is null.
///
/// # Safety
/// `mid` is a NUL-terminated string or null; `context` stays valid until `release`.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn teams_media_set_video_sink(
    slot: u32,
    mid: *const c_char,
    context: *mut c_void,
    callback: Option<TeamsVideoCallback>,
    release: Option<TeamsVideoRelease>,
) {
    let slot = match slot {
        0 => media::VideoSlot::Camera,
        1 => media::VideoSlot::Screen,
        _ if mid.is_null() => return,
        _ => media::VideoSlot::Remote(
            unsafe { CStr::from_ptr(mid) }
                .to_string_lossy()
                .into_owned(),
        ),
    };
    let sink = callback.map(|callback| {
        Arc::new(CSink {
            context,
            callback,
            release,
        }) as Arc<dyn media::VideoSink>
    });
    if sink.is_none()
        && let Some(release) = release
    {
        release(context);
    }
    let Some(engine) = CURRENT.lock().unwrap().as_ref().and_then(Weak::upgrade) else {
        return;
    };
    engine.engine.set_video_sink(slot, sink);
}

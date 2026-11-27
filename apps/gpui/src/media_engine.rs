//! Audio/video of the active call, driven by the core's `Media*` effects (`RustMediaEngine.swift`): our libwebrtc
//! engine (`teams_media`), or in demo mode a stand-in that pretends to negotiate. It speaks standard SDP; the core
//! converts to NGC.
use crate::call_report::Diagnostics;
use crate::media_devices::{self, MediaKind, permissions};
use crate::media_recorder::{LocalRecorder, PcmTap};
use crate::media_video::{FrameSlot, VideoFeed};
use crate::oxide::{Oxide, tr};
use futures::channel::mpsc::UnboundedSender;
use gpui::{Context, Task};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, SystemTime};
use teams_core::{Action, CallAction, Effect, IceServerView};
use teams_media::{MediaEngine, MediaEvent, PrepareOptions, ScreenSource, SendLimits, VideoSlot};

/// Which window shows a video, so a new frame redraws only that one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Window {
    Call,
    Main,
}

/// Work handed to the UI thread by the engine's and AVFoundation's threads.
pub enum MediaMsg {
    Frame(Window),
    /// What the engine reports, from one of its threads.
    Engine(MediaEvent),
    /// The microphone test's level, the end of its recording or playback.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    Tester(crate::media_tester::TesterMsg),
    /// The share sheet's sources or a thumbnail arrived.
    Picker(crate::call_share_picker::PickerMsg),
    /// CoreAudio devices or defaults changed.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    Devices,
}

/// The engine and what the call window shows of it.
pub struct Media {
    pub tx: UnboundedSender<MediaMsg>,
    /// Made for the first call (libwebrtc stays unloaded until then), kept for the next ones.
    engine: Option<MediaEngine>,
    /// Local camera and remote video, for the call window.
    pub local_video: Option<Rc<VideoFeed>>,
    pub remote_video: Option<Rc<VideoFeed>>,
    /// Remote video by m-line (`mid`): the mixer's cameras and shared screen.
    pub remote_videos: HashMap<String, Rc<VideoFeed>>,
    /// CSRCs heard in the mixed audio just now: who is speaking.
    pub speaking_sources: HashSet<u32>,
    /// Our microphone picks up a voice.
    pub local_speaking: bool,
    /// The media connection dropped during the call and is being restored.
    pub reconnecting: bool,
    /// What the engine saw during the current or last call.
    pub diagnostics: Diagnostics,
    /// What the next screen share captures, picked in the share sheet.
    pub chosen_screen: Option<ScreenSource>,
    pub recorder: Option<LocalRecorder>,
    /// The engine's PCM into `recorder`.
    pcm: Option<Arc<PcmTap>>,
    /// When the local recording (this computer only, audio) began.
    pub local_recording_started: Option<SystemTime>,
    /// The latest finished local recording (also when the call's end stopped it), until dismissed.
    pub last_local_recording: Option<PathBuf>,
    /// The call the engine works for.
    pub call_id: Option<String>,
    /// Demo mode's pretend negotiation.
    demo_timer: Option<Task<()>>,
}

impl Media {
    pub fn new(tx: UnboundedSender<MediaMsg>) -> Self {
        Self {
            tx,
            engine: None,
            local_video: None,
            remote_video: None,
            remote_videos: HashMap::new(),
            speaking_sources: HashSet::new(),
            local_speaking: false,
            reconnecting: false,
            diagnostics: Diagnostics::default(),
            chosen_screen: None,
            recorder: None,
            pcm: None,
            local_recording_started: None,
            last_local_recording: None,
            call_id: None,
            demo_timer: None,
        }
    }

    /// The engine, started on first use; its events come back on the UI thread.
    fn engine(&mut self) -> &MediaEngine {
        self.engine.get_or_insert_with(|| {
            install_media_log();
            let tx = self.tx.clone();
            MediaEngine::new(Arc::new(move |event: MediaEvent| {
                let _ = tx.unbounded_send(MediaMsg::Engine(event));
            }))
        })
    }

    /// Picks what the next screen share captures.
    pub fn choose_screen(&mut self, choice: Option<ScreenSource>) {
        self.chosen_screen = choice;
    }

    /// Shows a video slot that started, or forgets one that stopped (its frames stop with it).
    fn video(&mut self, slot: VideoSlot, present: bool) {
        // Our own screen is never shown: no sink, no conversion.
        if slot == VideoSlot::Screen {
            return;
        }
        if present {
            let shown = match &slot {
                VideoSlot::Camera => self.local_video.is_some(),
                VideoSlot::Remote(mid) => self.remote_videos.contains_key(mid),
                VideoSlot::Screen => true,
            };
            if shown {
                return;
            }
            let frames = FrameSlot::new(slot == VideoSlot::Camera, Window::Call, self.tx.clone());
            let feed = Rc::new(VideoFeed {
                slot: frames.clone(),
                source: slot.clone(),
            });
            self.engine().set_video_sink(
                slot.clone(),
                Some(frames as Arc<dyn teams_media::VideoSink>),
            );
            match slot {
                VideoSlot::Camera => self.local_video = Some(feed),
                VideoSlot::Remote(mid) => {
                    if self.remote_video.is_none() {
                        self.remote_video = Some(feed.clone());
                    }
                    self.remote_videos.insert(mid, feed);
                }
                VideoSlot::Screen => {}
            }
        } else {
            if let Some(engine) = &self.engine {
                engine.set_video_sink(slot.clone(), None);
            }
            match slot {
                VideoSlot::Camera => self.local_video = None,
                VideoSlot::Remote(mid) => {
                    let gone = self.remote_videos.remove(&mid);
                    if gone.is_some_and(|g| {
                        self.remote_video
                            .as_ref()
                            .is_some_and(|r| Rc::ptr_eq(r, &g))
                    }) {
                        self.remote_video = self.remote_videos.values().next().cloned();
                    }
                }
                VideoSlot::Screen => {}
            }
        }
    }

    /// Stops every video sink and forgets what the call showed.
    fn reset(&mut self) {
        let slots: Vec<VideoSlot> = self
            .local_video
            .iter()
            .chain(self.remote_videos.values())
            .map(|f| f.source.clone())
            .collect();
        if let Some(engine) = &self.engine {
            for slot in slots {
                engine.set_video_sink(slot, None);
            }
        }
        self.local_video = None;
        self.remote_video = None;
        self.remote_videos.clear();
        self.speaking_sources.clear();
        self.local_speaking = false;
        self.reconnecting = false;
    }

    fn end_call(&mut self) {
        stop_local_recording(self);
        self.chosen_screen = None;
        self.call_id = None;
        self.reset();
    }

    fn is_current(&self, call_id: &str) -> bool {
        self.call_id.as_deref() == Some(call_id)
    }
}

/// A line in `calling.log`, with the call's other progress lines.
pub fn trace(message: &str) {
    teams_calling::diagnostics::record(&format!("media {message}"));
}

/// The engine's progress lines (`log` records of `teams_media`) go to `calling.log` too.
struct MediaLog;

impl log::Log for MediaLog {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        metadata.target().starts_with("teams_media") && metadata.level() <= log::Level::Info
    }

    fn log(&self, record: &log::Record<'_>) {
        if self.enabled(record.metadata()) {
            trace(&record.args().to_string());
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

/// Tells the core what the engine saw.
fn emit(this: &Oxide, action: CallAction) {
    this.send(Action::Call { action });
}

/// Every `Media*` effect, as `Store.perform` routes them.
pub fn perform(this: &mut Oxide, effect: Effect, cx: &mut Context<Oxide>) {
    let demo = this.demo;
    match effect {
        Effect::MediaPrepare {
            call_id,
            ice_servers,
            video,
            muted,
            speaker,
            camera_lines,
            remote_offer,
        } => {
            if demo {
                demo_prepare(this, call_id, video, remote_offer.is_some(), cx);
            } else {
                let options = PrepareOptions {
                    call_id,
                    ice_servers: ice_servers.into_iter().map(ice_server).collect(),
                    video,
                    muted,
                    speaker,
                    camera_lines: camera_lines.max(0) as u32,
                    remote_offer,
                };
                prepare(this, options, cx);
            }
        }
        Effect::MediaRemoteDescription { call_id, sdp } => {
            remote_description(this, call_id, sdp, false, cx)
        }
        Effect::MediaRemoteProvisionalDescription { call_id, sdp } => {
            remote_description(this, call_id, sdp, true, cx)
        }
        Effect::MediaSetMuted { call_id, muted } if !demo => {
            let media = &mut this.ui.call.media;
            if media.is_current(&call_id) {
                media.engine().set_muted(&call_id, muted);
            }
        }
        Effect::MediaSetCamera { call_id, on } if !demo => set_camera(this, call_id, on, cx),
        Effect::MediaSetSpeaker { call_id, on } if !demo => {
            this.ui.call.media.engine().set_speaker(&call_id, on)
        }
        Effect::MediaSetScreenShare { call_id, on } if !demo => set_screen_share(this, call_id, on),
        Effect::MediaSetScreenShare { .. } => this.ui.call.media.choose_screen(None),
        Effect::MediaSendLimits {
            call_id,
            mid,
            sharing,
            max_fs,
            max_fps,
            max_bitrate,
            keyframe,
        } if !demo => {
            let limits = SendLimits {
                mid,
                sharing,
                max_fs,
                max_fps,
                max_bitrate,
                keyframe,
            };
            this.ui
                .call
                .media
                .engine()
                .apply_send_limits(&call_id, limits);
        }
        Effect::MediaRemoteOffer { call_id, sdp } => {
            if demo {
                emit(
                    this,
                    CallAction::RenegotiationAnswer {
                        call_id,
                        sdp: "v=0\r\no=- 0 1 IN IP4 127.0.0.1\r\ns=-\r\nt=0 0\r\n".into(),
                    },
                );
            } else {
                this.ui
                    .call
                    .media
                    .engine()
                    .answer_remote_offer(&call_id, &sdp);
            }
        }
        Effect::MediaOfferRejected { call_id, retry } if !demo => {
            this.ui.call.media.engine().offer_rejected(&call_id, retry)
        }
        Effect::MediaRestartIce { call_id } if !demo => {
            this.ui.call.media.engine().restart_ice(&call_id)
        }
        Effect::MediaClose { call_id } => {
            if demo {
                demo_close(this, &call_id);
            } else {
                close(this, &call_id);
            }
        }
        _ => {}
    }
}

fn ice_server(server: IceServerView) -> teams_media::IceServer {
    teams_media::IceServer {
        urls: server.urls,
        username: server.username,
        credential: server.credential,
    }
}

/// The devices chosen in Settings, for this call and the next ones.
fn apply_devices(this: &mut Oxide) {
    let devices = media_devices::engine_devices(&this.prefs, &this.ui.call.devices);
    this.ui.call.media.engine().set_devices(devices);
}

// MARK: Engine

/// Asks for the microphone (and the camera, for a video call), then opens the call's peer connection.
fn prepare(this: &mut Oxide, options: PrepareOptions, cx: &mut Context<Oxide>) {
    if let Some(previous) = this.ui.call.media.call_id.clone() {
        close(this, &previous);
    }
    let media = &mut this.ui.call.media;
    let call_id = options.call_id.clone();
    media.call_id = Some(call_id.clone());
    media.diagnostics = Diagnostics {
        camera_requested: options.video,
        ..Default::default()
    };
    cx.spawn(async move |this, cx| {
        if !permissions::request(MediaKind::Audio).await {
            let _ = this.update(cx, |this, cx| {
                this.ui.call.media.diagnostics.mic_permission_denied = true;
                fail(
                    this,
                    &call_id,
                    tr("ui.media_engine.microphone_access_is_denied_allow_oxide"),
                    cx,
                );
            });
            return;
        }
        let camera = options.video && permissions::request(MediaKind::Video).await;
        let _ = this.update(cx, |this, _| {
            if options.video && !camera {
                this.ui.call.media.diagnostics.camera_permission_denied = true;
            }
            if !this.ui.call.media.is_current(&call_id) {
                return;
            }
            apply_devices(this);
            this.ui.call.media.engine().prepare(PrepareOptions {
                video: camera,
                ..options
            });
        });
    })
    .detach();
}

fn remote_description(
    this: &mut Oxide,
    call_id: String,
    sdp: String,
    provisional: bool,
    cx: &mut Context<Oxide>,
) {
    if !this.demo {
        this.ui
            .call
            .media
            .engine()
            .set_remote_description(&call_id, &sdp, provisional);
        return;
    }
    if provisional {
        return;
    }
    this.ui.call.media.demo_timer = Some(cx.spawn(async move |this, cx| {
        cx.background_executor()
            .timer(Duration::from_millis(300))
            .await;
        let _ = this.update(cx, |this, cx| demo_connect(this, call_id, cx));
    }));
}

/// Mid-call camera: permission first, then the chosen camera, then the engine renegotiates.
fn set_camera(this: &mut Oxide, call_id: String, on: bool, cx: &mut Context<Oxide>) {
    if !this.ui.call.media.is_current(&call_id) {
        return;
    }
    cx.spawn(async move |this, cx| {
        let granted = !on || permissions::request(MediaKind::Video).await;
        let _ = this.update(cx, |this, _| {
            if !granted {
                this.ui.call.media.diagnostics.camera_permission_denied = true;
                return;
            }
            if on {
                apply_devices(this);
            }
            this.ui.call.media.engine().set_camera(&call_id, on);
        });
    })
    .detach();
}

/// Shares what the share sheet picked (else the main screen), or stops.
fn set_screen_share(this: &mut Oxide, call_id: String, on: bool) {
    if !this.ui.call.media.is_current(&call_id) {
        return;
    }
    if on && !permissions::screen_capture(true) {
        emit(
            this,
            CallAction::ScreenShareStopped {
                call_id,
                reason: Some(tr("ui.media_engine.allow_oxide_in_system_settings_privacy")),
            },
        );
        return;
    }
    let media = &mut this.ui.call.media;
    let source = media.chosen_screen.take().filter(|_| on);
    media.engine().set_screen_share(&call_id, on, source);
}

fn close(this: &mut Oxide, call_id: &str) {
    let media = &mut this.ui.call.media;
    if !media.is_current(call_id) {
        return;
    }
    media.engine().close(call_id);
    media.end_call();
}

fn fail(this: &mut Oxide, call_id: &str, reason: String, cx: &mut Context<Oxide>) {
    trace(&format!("failure: {reason}"));
    this.ui.call.media.diagnostics.failure_reason = Some(reason.clone());
    emit(
        this,
        CallAction::MediaFailed {
            call_id: call_id.to_owned(),
            reason,
        },
    );
    close(this, call_id);
    cx.notify();
}

/// What the engine reports, on the UI thread: the core's actions, and the call window's state.
fn on_event(this: &mut Oxide, event: MediaEvent, cx: &mut Context<Oxide>) {
    let media = &mut this.ui.call.media;
    match event {
        MediaEvent::LocalDescription { call_id, sdp } if media.is_current(&call_id) => {
            emit(this, CallAction::LocalDescription { call_id, sdp });
        }
        MediaEvent::RenegotiationAnswer { call_id, sdp } if media.is_current(&call_id) => {
            emit(this, CallAction::RenegotiationAnswer { call_id, sdp });
        }
        MediaEvent::Connected { call_id } if media.is_current(&call_id) => {
            media.diagnostics.connected = true;
            emit(this, CallAction::MediaConnected { call_id });
        }
        MediaEvent::Failed { call_id, reason } if media.is_current(&call_id) => {
            media.diagnostics.failure_reason = Some(reason.clone());
            media.end_call();
            emit(this, CallAction::MediaFailed { call_id, reason });
        }
        MediaEvent::ScreenShareStopped { call_id, reason } => {
            emit(this, CallAction::ScreenShareStopped { call_id, reason });
        }
        MediaEvent::CameraStopped { call_id, reason } => {
            emit(this, CallAction::CameraStopped { call_id, reason });
        }
        MediaEvent::Speaking { call_id, sources } if media.is_current(&call_id) => {
            let sources: HashSet<u32> = sources.into_iter().collect();
            if sources == media.speaking_sources {
                return;
            }
            media.speaking_sources = sources;
            crate::call_window::redraw(this, cx);
            return;
        }
        MediaEvent::LocalSpeaking { call_id, speaking } if media.is_current(&call_id) => {
            if media.local_speaking == speaking {
                return;
            }
            media.local_speaking = speaking;
            crate::call_window::redraw(this, cx);
            return;
        }
        MediaEvent::Reconnecting {
            call_id,
            reconnecting,
        } if media.is_current(&call_id) => {
            media.reconnecting = reconnecting;
        }
        MediaEvent::Video {
            call_id,
            slot,
            present,
        } if media.is_current(&call_id) => media.video(slot, present),
        MediaEvent::Diagnostics {
            call_id,
            diagnostics: sample,
        } if media.is_current(&call_id) => {
            merge(&mut media.diagnostics, &sample);
            return;
        }
        _ => return,
    }
    crate::call_window::redraw(this, cx);
    cx.notify();
}

fn merge(diagnostics: &mut Diagnostics, sample: &teams_media::CallDiagnostics) {
    diagnostics.camera_failed |= sample.camera_failed;
    diagnostics.disconnects = diagnostics.disconnects.max(sample.disconnects);
    diagnostics.merge(&Diagnostics {
        mic_energy: sample.mic_energy,
        audio_packets_received: sample.audio_packets_received as i64,
        audio_packets_lost: sample.audio_packets_lost as i64,
        round_trip: sample.round_trip,
        video_frames_sent: sample.video_frames_sent as i64,
        ..Default::default()
    });
}

// MARK: Demo

/// Demo mode: pretends to negotiate so the call flow can be exercised without a network.
fn demo_prepare(
    this: &mut Oxide,
    call_id: String,
    video: bool,
    answering: bool,
    cx: &mut Context<Oxide>,
) {
    let media = &mut this.ui.call.media;
    media.call_id = Some(call_id.clone());
    media.diagnostics = Diagnostics {
        camera_requested: video,
        ..Default::default()
    };
    let sdp = "v=0\r\no=- 0 0 IN IP4 127.0.0.1\r\ns=-\r\nt=0 0\r\nm=audio 9 UDP/TLS/RTP/SAVPF 111\r\na=rtpmap:111 opus/48000/2\r\n";
    media.demo_timer = Some(cx.spawn(async move |this, cx| {
        cx.background_executor()
            .timer(Duration::from_millis(300))
            .await;
        let _ = this.update(cx, |this, cx| {
            emit(
                this,
                CallAction::LocalDescription {
                    call_id: call_id.clone(),
                    sdp: sdp.into(),
                },
            );
            // Answering an incoming call: no remote answer follows, media is up now.
            if answering {
                demo_connect(this, call_id, cx);
            }
        });
    }));
}

fn demo_connect(this: &mut Oxide, call_id: String, cx: &mut Context<Oxide>) {
    let d = &mut this.ui.call.media.diagnostics;
    d.connected = true;
    d.mic_energy = 0.5;
    d.audio_packets_received = 500;
    d.video_frames_sent = if d.camera_requested { 300 } else { 0 };
    emit(this, CallAction::MediaConnected { call_id });
    cx.notify();
}

fn demo_close(this: &mut Oxide, call_id: &str) {
    let media = &mut this.ui.call.media;
    if !media.is_current(call_id) {
        return;
    }
    stop_local_recording(media);
    media.demo_timer = None;
    media.call_id = None;
}

// MARK: Local recording

/// Records the call's microphone and participants into `path` until stopped or the call closes.
pub fn start_local_recording(this: &mut Oxide, call_id: &str, path: PathBuf) -> Result<(), String> {
    let demo = this.demo;
    let media = &mut this.ui.call.media;
    if !media.is_current(call_id) {
        return Err("no call".into());
    }
    stop_local_recording(media);
    let recorder = LocalRecorder::new(path)?;
    media.local_recording_started = Some(recorder.started);
    if !demo {
        let tap = PcmTap::new(&recorder);
        media
            .engine()
            .set_pcm_sink(Some(tap.clone() as Arc<dyn teams_media::PcmSink>));
        media.pcm = Some(tap);
    }
    media.recorder = Some(recorder);
    trace("local recording started");
    Ok(())
}

/// Finishes the file; its location, or `None` when nothing was recording.
pub fn stop_local_recording(media: &mut Media) -> Option<PathBuf> {
    let recorder = media.recorder.take()?;
    if let Some(tap) = media.pcm.take() {
        if let Some(engine) = &media.engine {
            engine.set_pcm_sink(None);
        }
        tap.stop();
    }
    media.local_recording_started = None;
    trace("local recording stopped");
    let path = recorder.stop();
    media.last_local_recording = Some(path.clone());
    Some(path)
}

/// Messages from other threads, on the UI thread.
pub fn on_msg(this: &mut Oxide, msg: MediaMsg, cx: &mut Context<Oxide>) {
    match msg {
        MediaMsg::Frame(Window::Call) => crate::call_window::redraw(this, cx),
        MediaMsg::Frame(Window::Main) => cx.notify(),
        MediaMsg::Engine(event) => on_event(this, event, cx),
        MediaMsg::Tester(msg) => {
            #[cfg(target_os = "macos")]
            crate::media_tester::on_msg(this, msg, cx);
            #[cfg(not(target_os = "macos"))]
            let _ = msg;
        }
        MediaMsg::Picker(msg) => {
            crate::call_share_picker::on_msg(this, msg);
            crate::call_window::redraw(this, cx);
        }
        MediaMsg::Devices => {
            this.ui.call.devices.refresh();
            cx.notify();
            crate::call_window::redraw(this, cx);
        }
    }
}

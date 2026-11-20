//! Audio/video of the active call, driven by the core's `Media*` effects (`MediaEngine.swift`): libwebrtc
//! (`media_webrtc`), or in demo mode a stand-in that pretends to negotiate. It speaks standard SDP; the core
//! converts to NGC.
use crate::call_report::Diagnostics;
use crate::media_objc;
use crate::media_recorder::LocalRecorder;
use crate::media_video::VideoFeed;
use crate::media_webrtc::{self, Rtc, RtcMsg};
use crate::oxide::Oxide;
use futures::channel::mpsc::UnboundedSender;
use gpui::{Context, Task};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, SystemTime};
use teams_core::{Action, CallAction, Effect, IceServerView};

/// What the media engine reports back to the core.
pub enum MediaEvent {
    LocalDescription {
        call_id: String,
        sdp: String,
    },
    /// Answer to a renegotiation offer from the service.
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
    /// Screen sharing ended without the user asking (permission refused, capture stopped).
    ScreenShareStopped {
        call_id: String,
        reason: Option<String>,
    },
    /// The camera stopped without the user asking (the service rejected it).
    CameraStopped {
        call_id: String,
        reason: Option<String>,
    },
}

/// The mixer's bounds for one of our sending lines (`controlVideoStreaming` `fmtParams`).
#[derive(Clone, Debug, PartialEq)]
pub struct SendLimits {
    pub mid: String,
    pub sharing: bool,
    /// Macroblocks per frame.
    pub max_fs: u32,
    pub max_fps: f64,
    /// Bits per second; 0 leaves the encoder's own.
    pub max_bitrate: u32,
    pub keyframe: bool,
}

/// Which window shows a video, so a new frame redraws only that one.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Window {
    Call,
    Main,
}

/// Work handed to the UI thread by libwebrtc's and AVFoundation's threads.
pub enum MediaMsg {
    Frame(Window),
    FrameSize {
        slot: u64,
        width: i32,
        height: i32,
    },
    /// A callback of the peer connection built for generation `.0`.
    Rtc(u64, RtcMsg),
    /// The microphone test's level, the end of its recording or playback.
    Tester(crate::media_tester::TesterMsg),
    /// libwebrtc's lists of screens and windows changed (a thumbnail arrived).
    Desktop,
    /// CoreAudio devices or defaults changed.
    Devices,
}

/// A screen or window picked in the share sheet, with its lists (they must live until the capture starts).
pub struct ScreenChoice {
    pub source: media_objc::Obj,
    pub lists: Rc<crate::call_share_picker::DesktopSources>,
}

/// The engine and what the call window shows of it.
pub struct Media {
    pub tx: UnboundedSender<MediaMsg>,
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
    pub chosen_screen: Option<ScreenChoice>,
    pub recorder: Option<LocalRecorder>,
    /// When the local recording (this Mac only, audio) began.
    pub local_recording_started: Option<SystemTime>,
    /// The latest finished local recording (also when the call's end stopped it), until dismissed.
    pub last_local_recording: Option<PathBuf>,
    /// The call the engine works for.
    pub call_id: Option<String>,
    /// libwebrtc's side of it, built once permissions are granted and released by `close`.
    pub rtc: Option<Rtc>,
    /// Bumped for each peer connection, so late callbacks of an old one are ignored.
    pub generation: u64,
    /// Demo mode's pretend negotiation.
    demo_timer: Option<Task<()>>,
}

impl Media {
    pub fn new(tx: UnboundedSender<MediaMsg>) -> Self {
        Self {
            tx,
            local_video: None,
            remote_video: None,
            remote_videos: HashMap::new(),
            speaking_sources: HashSet::new(),
            local_speaking: false,
            reconnecting: false,
            diagnostics: Diagnostics::default(),
            chosen_screen: None,
            recorder: None,
            local_recording_started: None,
            last_local_recording: None,
            call_id: None,
            rtc: None,
            generation: 0,
            demo_timer: None,
        }
    }

    /// Picks what the next screen share captures; the lists of a previous pick are closed.
    pub fn choose_screen(&mut self, choice: Option<ScreenChoice>) {
        if let Some(old) = self.chosen_screen.take()
            && choice
                .as_ref()
                .is_none_or(|c| !Rc::ptr_eq(&c.lists, &old.lists))
        {
            old.lists.close();
        }
        self.chosen_screen = choice;
    }
}

/// Tells the core what the engine saw.
pub fn emit(this: &Oxide, event: MediaEvent) {
    let action = match event {
        MediaEvent::LocalDescription { call_id, sdp } => {
            CallAction::LocalDescription { call_id, sdp }
        }
        MediaEvent::RenegotiationAnswer { call_id, sdp } => {
            CallAction::RenegotiationAnswer { call_id, sdp }
        }
        MediaEvent::Connected { call_id } => CallAction::MediaConnected { call_id },
        MediaEvent::Failed { call_id, reason } => CallAction::MediaFailed { call_id, reason },
        MediaEvent::ScreenShareStopped { call_id, reason } => {
            CallAction::ScreenShareStopped { call_id, reason }
        }
        MediaEvent::CameraStopped { call_id, reason } => {
            CallAction::CameraStopped { call_id, reason }
        }
    };
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
                media_webrtc::prepare(
                    this,
                    call_id,
                    ice_servers,
                    video,
                    muted,
                    speaker,
                    camera_lines,
                    remote_offer,
                    cx,
                );
            }
        }
        Effect::MediaRemoteDescription { call_id, sdp } => {
            remote_description(this, call_id, sdp, false, cx)
        }
        Effect::MediaRemoteProvisionalDescription { call_id, sdp } => {
            remote_description(this, call_id, sdp, true, cx)
        }
        Effect::MediaSetMuted { call_id, muted } if !demo => {
            media_webrtc::set_muted(this, &call_id, muted)
        }
        Effect::MediaSetCamera { call_id, on } if !demo => {
            media_webrtc::set_camera(this, call_id, on, cx)
        }
        Effect::MediaSetSpeaker { call_id, on } if !demo => {
            media_webrtc::set_speaker(this, &call_id, on)
        }
        Effect::MediaSetScreenShare { call_id, on } if !demo => {
            media_webrtc::set_screen_share(this, call_id, on, cx)
        }
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
            media_webrtc::apply_send_limits(this, &call_id, limits, cx);
        }
        Effect::MediaRemoteOffer { call_id, sdp } => {
            if demo {
                emit(
                    this,
                    MediaEvent::RenegotiationAnswer {
                        call_id,
                        sdp: "v=0\r\no=- 0 1 IN IP4 127.0.0.1\r\ns=-\r\nt=0 0\r\n".into(),
                    },
                );
            } else {
                media_webrtc::answer_remote_offer(this, call_id, sdp);
            }
        }
        Effect::MediaOfferRejected { call_id, retry } if !demo => {
            media_webrtc::offer_rejected(this, call_id, retry)
        }
        Effect::MediaRestartIce { call_id } if !demo => media_webrtc::restart_ice(this, &call_id),
        Effect::MediaClose { call_id } => {
            if demo {
                demo_close(this, &call_id);
            } else {
                media_webrtc::close(this, &call_id);
            }
        }
        _ => {}
    }
}

fn remote_description(
    this: &mut Oxide,
    call_id: String,
    sdp: String,
    provisional: bool,
    cx: &mut Context<Oxide>,
) {
    if !this.demo {
        media_webrtc::set_remote_description(this, call_id, sdp, provisional);
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
                MediaEvent::LocalDescription {
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
    emit(this, MediaEvent::Connected { call_id });
    cx.notify();
}

fn demo_close(this: &mut Oxide, call_id: &str) {
    let media = &mut this.ui.call.media;
    if media.call_id.as_deref() != Some(call_id) {
        return;
    }
    stop_local_recording(media);
    media.demo_timer = None;
    media.call_id = None;
}

// MARK: Local recording

/// Records the call's microphone and participants into `path` (`.m4a`) until stopped or the call closes.
pub fn start_local_recording(this: &mut Oxide, call_id: &str, path: PathBuf) -> Result<(), String> {
    let media = &mut this.ui.call.media;
    if media.call_id.as_deref() != Some(call_id) {
        return Err("no call".into());
    }
    stop_local_recording(media);
    let recorder = LocalRecorder::new(path)?;
    media.local_recording_started = Some(recorder.started);
    if !this.demo {
        media_webrtc::record_into(media, &recorder);
    }
    media.recorder = Some(recorder);
    media_objc::trace("local recording started");
    Ok(())
}

/// Finishes the file; its location, or `None` when nothing was recording.
pub fn stop_local_recording(media: &mut Media) -> Option<PathBuf> {
    let recorder = media.recorder.take()?;
    media_webrtc::stop_recording_taps(media);
    media.local_recording_started = None;
    media_objc::trace("local recording stopped");
    let path = recorder.stop();
    media.last_local_recording = Some(path.clone());
    Some(path)
}

/// Messages from other threads, on the UI thread.
pub fn on_msg(this: &mut Oxide, msg: MediaMsg, cx: &mut Context<Oxide>) {
    match msg {
        MediaMsg::Frame(Window::Call) => crate::call_window::redraw(this, cx),
        MediaMsg::Frame(Window::Main) => cx.notify(),
        MediaMsg::FrameSize {
            slot,
            width,
            height,
        } => media_webrtc::frame_size(this, slot, width, height, cx),
        MediaMsg::Rtc(generation, msg) => media_webrtc::on_msg(this, generation, msg, cx),
        MediaMsg::Tester(msg) => crate::media_tester::on_msg(this, msg, cx),
        MediaMsg::Desktop => {
            if let Some(picker) = this.ui.call.picker.as_mut() {
                picker.refresh();
            }
            crate::call_window::redraw(this, cx);
        }
        MediaMsg::Devices => {
            this.ui.call.devices.refresh();
            cx.notify();
            crate::call_window::redraw(this, cx);
        }
    }
}

/// ICE servers as the core gives them, for the engine.
pub fn ice_server_count(servers: &[IceServerView]) -> (usize, bool) {
    (
        servers.len(),
        servers.iter().any(|s| s.credential.is_some()),
    )
}

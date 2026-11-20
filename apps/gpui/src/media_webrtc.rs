//! libwebrtc (LiveKit's build) for the active call, with the devices chosen in Settings: `WebRTCMediaEngine.swift`
//! and its `+Negotiation`, `+Audio`, `+Video` and `+ScreenShare` parts, call for call.
//!
//! libwebrtc answers on its own threads: its blocks and delegates only post a `RtcMsg` to the UI thread, tagged with
//! the generation of the peer connection they belong to; all state lives here, on the UI thread.
use crate::call_report::Diagnostics;
use crate::media_devices::{self, MediaKind, permissions};
use crate::media_engine::{Media, MediaEvent, MediaMsg, SendLimits, Window, emit};
use crate::media_objc::{self, CMFormatDescription, Obj, class, ns, string, trace};
use crate::media_recorder::{LocalRecorder, MicrophoneTap, RemoteAudioTap};
use crate::media_video::{FrameSlot, VideoFeed};
use crate::oxide::{Oxide, tr};
use block2::RcBlock;
use futures::channel::mpsc::UnboundedSender;
use gpui::{Context, Task};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool, NSObject};
use objc2::{AnyThread, DefinedClass, define_class, msg_send};
use std::collections::HashMap;
use std::rc::Rc;
use std::time::{Duration, Instant};
use teams_core::IceServerView;

// libwebrtc's enums (`RTCSignalingState`, `RTCPeerConnectionState`…), as NSInteger.
const SIGNALING_STABLE: isize = 0;
const SIGNALING_HAVE_LOCAL_OFFER: isize = 1;
const CONNECTION_CONNECTING: isize = 1;
const CONNECTION_CONNECTED: isize = 2;
const CONNECTION_DISCONNECTED: isize = 3;
const CONNECTION_FAILED: isize = 4;
const GATHERING_COMPLETE: isize = 2;
const SDP_OFFER: isize = 0;
const SDP_PRANSWER: isize = 1;
const SDP_ANSWER: isize = 2;
const SDP_ROLLBACK: isize = 3;
const DIRECTION_SEND_RECV: isize = 0;
const DIRECTION_SEND_ONLY: isize = 1;
const DIRECTION_RECV_ONLY: isize = 2;
const DIRECTION_INACTIVE: isize = 3;
const MEDIA_VIDEO: isize = 1;
const SOURCE_CSRC: isize = 1;

/// The web client's `reconnectTimeLimit`: how long a call that was connected tries to get its media back.
const RECONNECT_TIME_LIMIT: Duration = Duration::from_secs(90);

/// Which step of a negotiation a libwebrtc callback finishes.
#[derive(Clone, Debug)]
pub enum Purpose {
    /// The first offer, or the answer to an incoming call.
    Start,
    /// A new offer for the call as it now is.
    Renegotiate,
    /// The answer to the service's renegotiation offer.
    Answer,
    /// The caller's offer of an incoming call.
    IncomingOffer,
    /// The service's renegotiation offer.
    ServiceOffer,
    /// The answer to our offer.
    RemoteAnswer { provisional: bool },
}

/// What follows the rollback of a pending local offer.
#[derive(Clone, Debug)]
pub enum AfterRollback {
    Apply(String),
    Retry,
    Restore,
}

pub enum RtcMsg {
    Described {
        purpose: Purpose,
        result: Result<(isize, String), String>,
    },
    LocalSet {
        purpose: Purpose,
        error: Option<String>,
    },
    RemoteSet {
        purpose: Purpose,
        error: Option<String>,
    },
    RolledBack {
        error: Option<String>,
        then: AfterRollback,
    },
    GatheringComplete,
    ConnectionState(isize),
    AddedReceiver {
        receiver_id: String,
        mid: Option<String>,
    },
    Statistics(Diagnostics),
    LocalLevel(f64),
    CameraFailed,
    ScreenCaptureError,
}

/// Posts libwebrtc's callbacks to the UI thread.
#[derive(Clone)]
struct Post {
    tx: UnboundedSender<MediaMsg>,
    generation: u64,
}
impl Post {
    fn send(&self, msg: RtcMsg) {
        let _ = self.tx.unbounded_send(MediaMsg::Rtc(self.generation, msg));
    }
}

/// libwebrtc's side of one call. Built for a call and dropped by `close`: libwebrtc's threads, codecs and audio
/// device stay alive only during calls.
pub struct Rtc {
    post: Post,
    factory: Obj,
    /// Held: the processing module keeps its delegate weakly, and the factory its module.
    _processing: Obj,
    microphone_tap: Retained<MicrophoneTap>,
    delegate: Retained<PeerDelegate>,
    capture_delegate: Retained<CaptureDelegate>,
    connection: Option<Obj>,
    audio_track: Option<Obj>,
    camera_track: Option<Obj>,
    capturer: Option<Obj>,
    screen_capturer: Option<Obj>,
    screen_track: Option<Obj>,
    /// Follows the shared screen's frame size (the camera's comes with its feed).
    screen_probe: Option<Rc<VideoFeed>>,
    /// Frame size of our camera and screen tracks, by track id: the scale depends on it.
    frame_sizes: HashMap<String, (i32, i32)>,
    /// Last bounds the mixer set per line (WebRTC mid), applied again when the size changes.
    send_limits: HashMap<String, SendLimits>,
    /// The lines our camera and our screen go out on, kept from the offer that created them.
    camera_transceiver: Option<Obj>,
    sharing_transceiver: Option<Obj>,
    sent_description: bool,
    speaker_on: bool,
    answering_renegotiation: bool,
    gathering_timeout: Option<Task<()>>,
    connection_timeout: Option<Task<()>>,
    reconnection: Option<Task<()>>,
    stats_task: Option<Task<()>>,
    speaking_task: Option<Task<()>>,
    /// When each CSRC was last heard, so the indicator does not flicker between words.
    last_heard: HashMap<u32, Instant>,
    /// Each CSRC's packet timestamp at the previous poll: it moves only while that voice is mixed in.
    source_stamps: HashMap<u32, f64>,
    /// Renderers on the participants' audio tracks while recording.
    remote_taps: Vec<(Obj, Retained<RemoteAudioTap>)>,
}

fn constraints() -> Option<Obj> {
    let allocated = media_objc::alloc(c"LKRTCMediaConstraints")?;
    let none: Option<&AnyObject> = None;
    unsafe { msg_send![allocated, initWithMandatoryConstraints: none, optionalConstraints: none] }
}

fn session_description(kind: isize, sdp: &str) -> Option<Obj> {
    let allocated = media_objc::alloc(c"LKRTCSessionDescription")?;
    let sdp = ns(sdp);
    unsafe { msg_send![allocated, initWithType: kind, sdp: &*sdp] }
}

fn ssl() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        if let Some(pointer) = media_objc::symbol(c"LKRTCInitializeSSL") {
            let initialize: extern "C" fn() -> Bool = unsafe { std::mem::transmute(pointer) };
            initialize();
        }
    });
}

impl Rtc {
    fn new(post: Post) -> Option<Self> {
        ssl();
        // The microphone after processing goes through `microphone_tap`, for the local recording. Set through the
        // property: this build ignores the delegate given to the initializer.
        let none: Option<&AnyObject> = None;
        let processing: Obj = unsafe {
            msg_send![
                media_objc::alloc(c"LKRTCDefaultAudioProcessingModule")?,
                initWithConfig: none,
                capturePostProcessingDelegate: none,
                renderPreProcessingDelegate: none
            ]
        };
        let microphone_tap = MicrophoneTap::new();
        unsafe {
            let _: () = msg_send![&*processing, setCapturePostProcessingDelegate: &*microphone_tap];
        }
        let encoder = media_objc::new(c"LKRTCDefaultVideoEncoderFactory")?;
        let decoder = media_objc::new(c"LKRTCDefaultVideoDecoderFactory")?;
        let factory: Obj = unsafe {
            msg_send![
                media_objc::alloc(c"LKRTCPeerConnectionFactory")?,
                initWithAudioDeviceModuleType: 0isize,
                bypassVoiceProcessing: false,
                encoderFactory: &*encoder,
                decoderFactory: &*decoder,
                audioProcessingModule: &*processing
            ]
        };
        Some(Self {
            delegate: PeerDelegate::new(post.clone()),
            capture_delegate: CaptureDelegate::new(post.clone()),
            post,
            factory,
            _processing: processing,
            microphone_tap,
            connection: None,
            audio_track: None,
            camera_track: None,
            capturer: None,
            screen_capturer: None,
            screen_track: None,
            screen_probe: None,
            frame_sizes: HashMap::new(),
            send_limits: HashMap::new(),
            camera_transceiver: None,
            sharing_transceiver: None,
            sent_description: false,
            speaker_on: true,
            answering_renegotiation: false,
            gathering_timeout: None,
            connection_timeout: None,
            reconnection: None,
            stats_task: None,
            speaking_task: None,
            last_heard: HashMap::new(),
            source_stamps: HashMap::new(),
            remote_taps: Vec::new(),
        })
    }

    fn connection(&self) -> Option<&AnyObject> {
        self.connection.as_deref()
    }
}

fn signaling_state(connection: &AnyObject) -> isize {
    unsafe { msg_send![connection, signalingState] }
}
fn connection_state(connection: &AnyObject) -> isize {
    unsafe { msg_send![connection, connectionState] }
}
fn transceivers(connection: &AnyObject) -> Vec<Obj> {
    let list: Option<Obj> = unsafe { msg_send![connection, transceivers] };
    media_objc::array(list.as_deref())
}
fn receivers(connection: &AnyObject) -> Vec<Obj> {
    let list: Option<Obj> = unsafe { msg_send![connection, receivers] };
    media_objc::array(list.as_deref())
}
fn receiver_track(receiver: &AnyObject) -> Option<Obj> {
    unsafe { msg_send![receiver, track] }
}
fn kind(track: &AnyObject) -> String {
    let kind: Option<Obj> = unsafe { msg_send![track, kind] };
    string(kind.as_deref())
}
fn track_id(track: &AnyObject) -> String {
    let id: Option<Obj> = unsafe { msg_send![track, trackId] };
    string(id.as_deref())
}
fn set_enabled(track: &AnyObject, on: bool) {
    unsafe {
        let _: () = msg_send![track, setIsEnabled: on];
    }
}
fn is_enabled(track: &AnyObject) -> bool {
    unsafe { msg_send![track, isEnabled] }
}
fn mid(transceiver: &AnyObject) -> String {
    let mid: Option<Obj> = unsafe { msg_send![transceiver, mid] };
    string(mid.as_deref())
}
fn is_stopped(transceiver: &AnyObject) -> bool {
    unsafe { msg_send![transceiver, isStopped] }
}
fn sender(transceiver: &AnyObject) -> Option<Obj> {
    unsafe { msg_send![transceiver, sender] }
}
fn sender_track(transceiver: &AnyObject) -> Option<Obj> {
    let sender = sender(transceiver)?;
    unsafe { msg_send![&*sender, track] }
}
fn set_sender_track(transceiver: &AnyObject, track: Option<&AnyObject>) {
    if let Some(sender) = sender(transceiver) {
        unsafe {
            let _: () = msg_send![&*sender, setTrack: track];
        }
    }
}
fn set_direction(transceiver: &AnyObject, direction: isize) {
    unsafe {
        let _: () = msg_send![transceiver, setDirection: direction, error: std::ptr::null_mut::<*mut AnyObject>()];
    }
}
fn audio_tracks(connection: &AnyObject) -> Vec<Obj> {
    receivers(connection)
        .iter()
        .filter_map(|r| receiver_track(r))
        .filter(|t| kind(t) == "audio")
        .collect()
}

// MARK: Lifecycle

#[allow(clippy::too_many_arguments)]
pub fn prepare(
    this: &mut Oxide,
    call_id: String,
    ice_servers: Vec<IceServerView>,
    video: bool,
    muted: bool,
    speaker: bool,
    camera_lines: u32,
    remote_offer: Option<String>,
    cx: &mut Context<Oxide>,
) {
    trace(&format!(
        "prepare servers={} TURN={} incoming={}",
        ice_servers.len(),
        ice_servers.iter().any(|s| s.credential.is_some()),
        remote_offer.is_some()
    ));
    let current = this
        .ui
        .call
        .media
        .call_id
        .clone()
        .unwrap_or_else(|| call_id.clone());
    close(this, &current);
    let media = &mut this.ui.call.media;
    media.call_id = Some(call_id.clone());
    media.diagnostics = Diagnostics {
        camera_requested: video,
        ..Default::default()
    };
    if !media_objc::available() {
        fail(
            this,
            &call_id,
            tr("ui.media_engine.couldn_t_prepare_the_audio_connection"),
        );
        return;
    }
    cx.spawn(async move |this, cx| {
        let microphone = permissions::request(MediaKind::Audio).await;
        if !microphone {
            let _ = this.update(cx, |this, _| {
                this.ui.call.media.diagnostics.mic_permission_denied = true;
                fail(
                    this,
                    &call_id,
                    tr("ui.media_engine.microphone_access_is_denied_allow_oxide"),
                );
            });
            return;
        }
        let camera = video && permissions::request(MediaKind::Video).await;
        let _ = this.update(cx, |this, cx| {
            if video && !camera {
                this.ui.call.media.diagnostics.camera_permission_denied = true;
            }
            if this.ui.call.media.call_id.as_deref() != Some(&call_id) {
                return;
            }
            start(
                this,
                call_id,
                ice_servers,
                camera,
                muted,
                speaker,
                camera_lines,
                remote_offer,
                cx,
            );
        });
    })
    .detach();
}

pub fn close(this: &mut Oxide, call_id: &str) {
    let media = &mut this.ui.call.media;
    if media.call_id.as_deref() != Some(call_id) {
        return;
    }
    crate::media_engine::stop_local_recording(media);
    media.reconnecting = false;
    if let Some(rtc) = media.rtc.take() {
        stop_camera(&rtc);
        stop_screen_capture_of(&rtc);
        if let Some(connection) = rtc.connection() {
            unsafe {
                let _: () = msg_send![connection, close];
            }
        }
        // Dropping `rtc` cancels its timers and releases the factory: one call at a time.
    }
    media.choose_screen(None);
    media.local_video = None;
    media.remote_video = None;
    media.remote_videos.clear();
    media.speaking_sources.clear();
    media.local_speaking = false;
    media.call_id = None;
}

fn fail(this: &mut Oxide, call_id: &str, reason: String) {
    trace(&format!("failure: {reason}"));
    this.ui.call.media.diagnostics.failure_reason = Some(reason.clone());
    emit(
        this,
        MediaEvent::Failed {
            call_id: call_id.to_owned(),
            reason,
        },
    );
    close(this, call_id);
}

/// The engine's state for this call and generation, if still current.
fn current(media: &mut Media, generation: u64) -> Option<(&mut Rtc, String)> {
    let call_id = media.call_id.clone()?;
    let rtc = media
        .rtc
        .as_mut()
        .filter(|r| r.post.generation == generation)?;
    Some((rtc, call_id))
}

// MARK: Negotiation

#[allow(clippy::too_many_arguments)]
fn start(
    this: &mut Oxide,
    call_id: String,
    ice_servers: Vec<IceServerView>,
    video: bool,
    muted: bool,
    speaker: bool,
    camera_lines: u32,
    remote_offer: Option<String>,
    cx: &mut Context<Oxide>,
) {
    let media = &mut this.ui.call.media;
    media.generation += 1;
    let post = Post {
        tx: media.tx.clone(),
        generation: media.generation,
    };
    let Some(mut rtc) = Rtc::new(post) else {
        fail(
            this,
            &call_id,
            tr("ui.media_engine.couldn_t_prepare_the_audio_connection"),
        );
        return;
    };
    rtc.speaker_on = speaker;
    apply_audio_devices(&rtc, &this.prefs);
    let Some(connection) = make_connection(&rtc, &ice_servers) else {
        fail(
            this,
            &call_id,
            tr("ui.media_engine.couldn_t_prepare_the_audio_connection"),
        );
        return;
    };
    let offering = remote_offer.is_none();
    let Some(constraints) = constraints() else {
        return;
    };
    let source: Obj =
        unsafe { msg_send![&*rtc.factory, audioSourceWithConstraints: &*constraints] };
    let audio: Obj = unsafe {
        msg_send![&*rtc.factory, audioTrackWithSource: &*source, trackId: &*ns("audio0")]
    };
    add(&audio, &connection, offering);
    set_enabled(&audio, !muted);
    rtc.microphone_tap.set_muted(muted);
    rtc.audio_track = Some(audio);
    rtc.connection = Some(connection.clone());
    let media = &mut this.ui.call.media;
    if video && let Some(track) = make_camera_track(&mut rtc, media, &this.prefs) {
        add(&track, &connection, offering);
        rtc.camera_transceiver = transceivers(&connection)
            .into_iter()
            .find(|t| sender_track(t).is_some_and(|s| media_objc::same(&s, &track)));
    }
    if offering {
        // Like the web client, always offer every camera line and the sharing line: the mixer rejects missing ones.
        let first = if rtc.camera_track.is_some() { 1 } else { 0 };
        for _ in first..camera_lines.max(1) {
            let line = add_receive_video(&connection);
            if rtc.camera_transceiver.is_none() {
                rtc.camera_transceiver = line;
            }
        }
        rtc.sharing_transceiver = add_receive_video(&connection);
    }
    let post = rtc.post.clone();
    media.rtc = Some(rtc);
    match remote_offer {
        Some(offer) => set_remote(
            &connection,
            SDP_OFFER,
            &offer,
            Purpose::IncomingOffer,
            &post,
        ),
        None => describe(&connection, true, Purpose::Start, &post),
    }
    let _ = cx;
}

fn make_connection(rtc: &Rtc, ice_servers: &[IceServerView]) -> Option<Obj> {
    let config = media_objc::new(c"LKRTCConfiguration")?;
    let servers: Vec<Obj> = ice_servers
        .iter()
        .filter_map(|server| {
            let urls = media_objc::nsarray_of_strings(&server.urls);
            let username = server.username.as_deref().map(ns);
            let credential = server.credential.as_deref().map(ns);
            unsafe {
                msg_send![
                    media_objc::alloc(c"LKRTCIceServer")?,
                    initWithURLStrings: &*urls,
                    username: username.as_deref(),
                    credential: credential.as_deref()
                ]
            }
        })
        .collect();
    let servers: Vec<&AnyObject> = servers.iter().map(|s| &**s).collect();
    let servers = media_objc::nsarray(&servers);
    unsafe {
        let _: () = msg_send![&*config, setIceServers: &*servers];
        let _: () = msg_send![&*config, setSdpSemantics: 1isize];
        let _: () = msg_send![&*config, setBundlePolicy: 2isize];
        let _: () = msg_send![&*config, setRtcpMuxPolicy: 1isize];
        let _: () = msg_send![&*config, setContinualGatheringPolicy: 0isize];
    }
    let constraints = constraints()?;
    unsafe {
        msg_send![
            &*rtc.factory,
            peerConnectionWithConfiguration: &*config,
            constraints: &*constraints,
            delegate: &*rtc.delegate
        ]
    }
}

fn add(track: &AnyObject, connection: &AnyObject, offering: bool) {
    let streams = media_objc::nsarray_of_strings(&["local".to_owned()]);
    if !offering {
        let _: Option<Obj> =
            unsafe { msg_send![connection, addTrack: track, streamIds: &*streams] };
        return;
    }
    let Some(options) = media_objc::new(c"LKRTCRtpTransceiverInit") else {
        return;
    };
    unsafe {
        let _: () = msg_send![&*options, setDirection: DIRECTION_SEND_RECV];
        let _: () = msg_send![&*options, setStreamIds: &*streams];
        let _: Option<Obj> = msg_send![connection, addTransceiverWithTrack: track, init: &*options];
    }
}

fn add_receive_video(connection: &AnyObject) -> Option<Obj> {
    let options = media_objc::new(c"LKRTCRtpTransceiverInit")?;
    unsafe {
        let _: () = msg_send![&*options, setDirection: DIRECTION_RECV_ONLY];
        msg_send![connection, addTransceiverOfType: MEDIA_VIDEO, init: &*options]
    }
}

/// Creates an offer (or answer); the description comes back as `RtcMsg::Described`.
fn describe(connection: &AnyObject, offer: bool, purpose: Purpose, post: &Post) {
    let Some(constraints) = constraints() else {
        return;
    };
    let post = post.clone();
    let block = RcBlock::new(move |description: *mut AnyObject, error: *mut AnyObject| {
        let result = match unsafe { description.as_ref() } {
            Some(description) if error.is_null() => {
                let kind: isize = unsafe { msg_send![description, type] };
                let sdp: Option<Obj> = unsafe { msg_send![description, sdp] };
                Ok((kind, string(sdp.as_deref())))
            }
            _ => Err(media_objc::error_text(error)),
        };
        post.send(RtcMsg::Described {
            purpose: purpose.clone(),
            result,
        });
    });
    unsafe {
        if offer {
            let _: () = msg_send![connection, offerForConstraints: &*constraints, completionHandler: &*block];
        } else {
            let _: () = msg_send![connection, answerForConstraints: &*constraints, completionHandler: &*block];
        }
    }
}

fn completion(
    post: &Post,
    done: impl Fn(Option<String>) -> RtcMsg + 'static,
) -> RcBlock<dyn Fn(*mut AnyObject)> {
    let post = post.clone();
    RcBlock::new(move |error: *mut AnyObject| {
        let error = (!error.is_null()).then(|| media_objc::error_text(error));
        post.send(done(error));
    })
}

fn set_local(connection: &AnyObject, kind: isize, sdp: &str, purpose: Purpose, post: &Post) {
    let Some(description) = session_description(kind, sdp) else {
        return;
    };
    let block = completion(post, move |error| RtcMsg::LocalSet {
        purpose: purpose.clone(),
        error,
    });
    unsafe {
        let _: () =
            msg_send![connection, setLocalDescription: &*description, completionHandler: &*block];
    }
}

fn set_remote(connection: &AnyObject, kind: isize, sdp: &str, purpose: Purpose, post: &Post) {
    let Some(description) = session_description(kind, sdp) else {
        return;
    };
    let block = completion(post, move |error| RtcMsg::RemoteSet {
        purpose: purpose.clone(),
        error,
    });
    unsafe {
        let _: () =
            msg_send![connection, setRemoteDescription: &*description, completionHandler: &*block];
    }
}

/// A new local offer for the call as it now is: the core sends it as a renegotiation.
fn renegotiate(rtc: &Rtc, reason: &str) {
    let Some(connection) = rtc.connection() else {
        return;
    };
    trace(&format!("renegotiate: {reason}"));
    describe(connection, true, Purpose::Renegotiate, &rtc.post);
}

/// NGC takes no trickled candidates: send once gathering completes, or after 3 s with what was gathered.
fn wait_for_gathering(this: &mut Oxide, generation: u64, cx: &mut Context<Oxide>) {
    let Some((rtc, _)) = current(&mut this.ui.call.media, generation) else {
        return;
    };
    let complete = rtc.connection().is_some_and(|c| {
        let state: isize = unsafe { msg_send![c, iceGatheringState] };
        state == GATHERING_COMPLETE
    });
    if complete {
        send_description(this, generation, cx);
        return;
    }
    rtc.gathering_timeout = Some(cx.spawn(async move |this, cx| {
        cx.background_executor().timer(Duration::from_secs(3)).await;
        let _ = this.update(cx, |this, cx| send_description(this, generation, cx));
    }));
}

fn send_description(this: &mut Oxide, generation: u64, cx: &mut Context<Oxide>) {
    let Some((rtc, call_id)) = current(&mut this.ui.call.media, generation) else {
        return;
    };
    if rtc.sent_description {
        return;
    }
    let Some(connection) = rtc.connection.clone() else {
        return;
    };
    let local: Option<Obj> = unsafe { msg_send![&*connection, localDescription] };
    let Some(local) = local else { return };
    let sdp: Option<Obj> = unsafe { msg_send![&*local, sdp] };
    let sdp = string(sdp.as_deref());
    let kind: isize = unsafe { msg_send![&*local, type] };
    rtc.sent_description = true;
    let gathering: isize = unsafe { msg_send![&*connection, iceGatheringState] };
    trace(&format!(
        "local SDP candidates={} relay={} gathering={gathering}",
        sdp.matches("a=candidate:").count(),
        sdp.contains(" typ relay")
    ));
    rtc.gathering_timeout = None;
    let answering = std::mem::take(&mut rtc.answering_renegotiation);
    // An incoming call (local answer) starts the deadline here; an outgoing one when the remote answer lands.
    if kind == SDP_ANSWER {
        wait_for_connection(this, generation, cx);
    }
    if answering {
        emit(this, MediaEvent::RenegotiationAnswer { call_id, sdp });
    } else {
        emit(this, MediaEvent::LocalDescription { call_id, sdp });
    }
}

pub fn set_remote_description(this: &mut Oxide, call_id: String, sdp: String, provisional: bool) {
    trace(&format!(
        "remote SDP provisional={provisional} candidates={}",
        sdp.matches("a=candidate:").count()
    ));
    let media = &mut this.ui.call.media;
    if media.call_id.as_deref() != Some(&call_id) {
        return;
    }
    let Some(rtc) = media.rtc.as_ref() else {
        return;
    };
    let Some(connection) = rtc.connection() else {
        return;
    };
    let kind = if provisional {
        SDP_PRANSWER
    } else {
        SDP_ANSWER
    };
    set_remote(
        connection,
        kind,
        &sdp,
        Purpose::RemoteAnswer { provisional },
        &rtc.post,
    );
}

fn wait_for_connection(this: &mut Oxide, generation: u64, cx: &mut Context<Oxide>) {
    let Some((rtc, call_id)) = current(&mut this.ui.call.media, generation) else {
        return;
    };
    rtc.connection_timeout = None;
    // While reconnecting, `reconnect` owns the deadline.
    let connected = rtc
        .connection()
        .is_some_and(|c| connection_state(c) == CONNECTION_CONNECTED);
    if connected || rtc.reconnection.is_some() {
        return;
    }
    rtc.connection_timeout = Some(cx.spawn(async move |this, cx| {
        cx.background_executor()
            .timer(Duration::from_secs(30))
            .await;
        let _ = this.update(cx, |this, _| {
            let Some((rtc, _)) = current(&mut this.ui.call.media, generation) else {
                return;
            };
            if rtc
                .connection()
                .is_some_and(|c| connection_state(c) == CONNECTION_CONNECTED)
            {
                return;
            }
            this.ui.call.media.diagnostics.failed = true;
            fail(
                this,
                &call_id,
                tr("ui.media_engine.media_negotiation_succeeded_but_the_audio"),
            );
        });
    }));
}

/// Our own offer may still be pending, e.g. a camera renegotiation the service refused (`mediaRejection`):
/// a connection takes one offer at a time, so ours yields to the service's instead of failing the call.
pub fn answer_remote_offer(this: &mut Oxide, call_id: String, sdp: String) {
    roll_back(this, &call_id, AfterRollback::Apply(sdp));
}

/// The service rejected our renegotiation: drop our offer, then offer again, or put the senders back
/// as last negotiated, like the web client's `rejectNegotiation`.
pub fn offer_rejected(this: &mut Oxide, call_id: String, retry: bool) {
    trace(&format!("local offer rejected retry={retry}"));
    roll_back(
        this,
        &call_id,
        if retry {
            AfterRollback::Retry
        } else {
            AfterRollback::Restore
        },
    );
}

fn roll_back(this: &mut Oxide, call_id: &str, then: AfterRollback) {
    let media = &mut this.ui.call.media;
    if media.call_id.as_deref() != Some(call_id) {
        return;
    }
    let Some(rtc) = media.rtc.as_ref() else {
        return;
    };
    let Some(connection) = rtc.connection.clone() else {
        return;
    };
    let generation = rtc.post.generation;
    if signaling_state(&connection) != SIGNALING_HAVE_LOCAL_OFFER {
        after_rollback(this, generation, then);
        return;
    }
    trace("rolling back the pending local offer");
    let Some(description) = session_description(SDP_ROLLBACK, "") else {
        return;
    };
    let block = completion(&rtc.post, move |error| RtcMsg::RolledBack {
        error,
        then: then.clone(),
    });
    unsafe {
        let _: () =
            msg_send![&*connection, setLocalDescription: &*description, completionHandler: &*block];
    }
}

fn after_rollback(this: &mut Oxide, generation: u64, then: AfterRollback) {
    let Some((rtc, call_id)) = current(&mut this.ui.call.media, generation) else {
        return;
    };
    match then {
        AfterRollback::Apply(sdp) => {
            if let Some(connection) = rtc.connection() {
                set_remote(
                    connection,
                    SDP_OFFER,
                    &sdp,
                    Purpose::ServiceOffer,
                    &rtc.post,
                );
            }
        }
        AfterRollback::Retry => renegotiate(rtc, "retry after rejection"),
        AfterRollback::Restore => restore_negotiated_senders(this, generation, call_id),
    }
}

/// Stops what the rejected offer meant to start sending, and tells the core.
fn restore_negotiated_senders(this: &mut Oxide, generation: u64, call_id: String) {
    let media = &mut this.ui.call.media;
    let Some((rtc, _)) = current(media, generation) else {
        return;
    };
    let mut events = Vec::new();
    if rtc.camera_track.is_some() && !sends(rtc.camera_transceiver.as_deref()) {
        stop_camera(rtc);
        if let Some(line) = rtc.camera_transceiver.as_deref().filter(|l| !is_stopped(l)) {
            set_sender_track(line, None);
            set_direction(line, DIRECTION_RECV_ONLY);
        }
        rtc.camera_track = None;
        events.push(MediaEvent::CameraStopped {
            call_id: call_id.clone(),
            reason: Some(tr("ui.media_engine.the_service_refused_the_camera")),
        });
    }
    if rtc.screen_track.is_some() && !sends(rtc.sharing_transceiver.as_deref()) {
        stop_screen_capture(rtc);
        if let Some(line) = rtc
            .sharing_transceiver
            .as_deref()
            .filter(|l| !is_stopped(l))
        {
            set_sender_track(line, None);
            set_direction(line, DIRECTION_RECV_ONLY);
        }
        events.push(MediaEvent::ScreenShareStopped {
            call_id,
            reason: Some(tr("ui.media_engine.the_service_refused_screen_sharing")),
        });
    }
    if events
        .iter()
        .any(|e| matches!(e, MediaEvent::CameraStopped { .. }))
    {
        media.local_video = None;
    }
    for event in events {
        emit(this, event);
    }
}

/// The last negotiation lets this line send.
fn sends(line: Option<&AnyObject>) -> bool {
    let Some(line) = line.filter(|l| !is_stopped(l)) else {
        return false;
    };
    let mut current = DIRECTION_INACTIVE;
    let known: bool = unsafe { msg_send![line, currentDirection: &raw mut current] };
    known && (current == DIRECTION_SEND_RECV || current == DIRECTION_SEND_ONLY)
}

pub fn restart_ice(this: &mut Oxide, call_id: &str) {
    let media = &mut this.ui.call.media;
    if media.call_id.as_deref() != Some(call_id) {
        return;
    }
    let Some(rtc) = media.rtc.as_ref() else {
        return;
    };
    let Some(connection) = rtc.connection() else {
        return;
    };
    unsafe {
        let _: () = msg_send![connection, restartIce];
    }
    renegotiate(rtc, "ICE restart");
}

/// The connection failed after being up, e.g. a Wi-Fi drop: restart ICE, again after 1 s, 2 s… up to 5 s
/// between attempts, and end the call only if nothing comes back within `RECONNECT_TIME_LIMIT`,
/// like the web client (`reconnectTimeLimit`, `maxReconnectDelayTime`).
fn reconnect(this: &mut Oxide, generation: u64, cx: &mut Context<Oxide>) {
    let media = &mut this.ui.call.media;
    let Some((rtc, call_id)) = current(media, generation) else {
        return;
    };
    if rtc.reconnection.is_some() {
        return;
    }
    trace(&format!(
        "reconnecting for up to {}s",
        RECONNECT_TIME_LIMIT.as_secs()
    ));
    let deadline = Instant::now() + RECONNECT_TIME_LIMIT;
    rtc.reconnection = Some(cx.spawn(async move |this, cx| {
        let mut attempt = 0u64;
        while Instant::now() < deadline {
            let pause = this.update(cx, |this, _| {
                let (rtc, _) = current(&mut this.ui.call.media, generation)?;
                let connection = rtc.connection.clone()?;
                // No offer in flight and ICE no longer checking: the previous attempt is over without success.
                let state = connection_state(&connection);
                let settled = signaling_state(&connection) == SIGNALING_STABLE
                    && state != CONNECTION_CONNECTING
                    && state != CONNECTION_CONNECTED;
                if !settled {
                    return Some(Duration::from_millis(500));
                }
                attempt += 1;
                trace(&format!("reconnect attempt {attempt}"));
                let call_id = this.ui.call.media.call_id.clone()?;
                restart_ice(this, &call_id);
                Some(Duration::from_secs(attempt.min(5)))
            });
            let Ok(Some(pause)) = pause else { return };
            let left = deadline.saturating_duration_since(Instant::now());
            cx.background_executor().timer(pause.min(left)).await;
        }
        let _ = this.update(cx, |this, _| {
            if current(&mut this.ui.call.media, generation).is_none() {
                return;
            }
            this.ui.call.media.diagnostics.failed = true;
            fail(
                this,
                &call_id,
                tr("ui.media_engine.the_audio_video_connection_failed_network"),
            );
        });
    }));
    media.reconnecting = true;
}

/// libwebrtc's callbacks, on the UI thread.
pub fn on_msg(this: &mut Oxide, generation: u64, msg: RtcMsg, cx: &mut Context<Oxide>) {
    let Some((rtc, call_id)) = current(&mut this.ui.call.media, generation) else {
        return;
    };
    let Some(connection) = rtc.connection.clone() else {
        return;
    };
    match msg {
        RtcMsg::Described { purpose, result } => match result {
            Ok((kind, sdp)) => set_local(&connection, kind, &sdp, purpose, &rtc.post),
            Err(error) => {
                if matches!(purpose, Purpose::Start) {
                    fail(
                        this,
                        &call_id,
                        tr("ui.media_engine.couldn_t_create_the_local_media") + &error,
                    );
                }
            }
        },
        RtcMsg::LocalSet { purpose, error } => match (purpose, error) {
            (Purpose::Start, Some(error)) => fail(
                this,
                &call_id,
                tr("ui.media_engine.local_description_rejected") + &error,
            ),
            (Purpose::Start, None) => wait_for_gathering(this, generation, cx),
            (purpose @ (Purpose::Renegotiate | Purpose::Answer), None) => {
                rtc.sent_description = false;
                rtc.answering_renegotiation = matches!(purpose, Purpose::Answer);
                wait_for_gathering(this, generation, cx);
            }
            _ => {}
        },
        RtcMsg::RemoteSet { purpose, error } => match (purpose, error) {
            (Purpose::IncomingOffer, Some(error)) => fail(
                this,
                &call_id,
                tr("ui.media_engine.remote_offer_rejected") + &error,
            ),
            (Purpose::IncomingOffer, None) => {
                describe(&connection, false, Purpose::Start, &rtc.post)
            }
            (Purpose::ServiceOffer, Some(error)) => fail(
                this,
                &call_id,
                tr("ui.media_engine.renegotiation_rejected") + &error,
            ),
            (Purpose::ServiceOffer, None) => {
                describe(&connection, false, Purpose::Answer, &rtc.post)
            }
            (Purpose::RemoteAnswer { .. }, Some(error)) => fail(
                this,
                &call_id,
                tr("ui.media_engine.media_answer_rejected") + &error,
            ),
            (Purpose::RemoteAnswer { provisional }, None) => {
                trace("remote SDP accepted");
                if !provisional {
                    wait_for_connection(this, generation, cx);
                }
            }
            _ => {}
        },
        RtcMsg::RolledBack {
            error: Some(error), ..
        } => fail(
            this,
            &call_id,
            tr("ui.media_engine.renegotiation_rejected") + &error,
        ),
        RtcMsg::RolledBack { error: None, then } => after_rollback(this, generation, then),
        RtcMsg::GatheringComplete => send_description(this, generation, cx),
        RtcMsg::ConnectionState(state) => connection_changed(this, generation, call_id, state, cx),
        RtcMsg::AddedReceiver { receiver_id, mid } => {
            let Some(receiver) = receivers(&connection).into_iter().find(|r| {
                let id: Option<Obj> = unsafe { msg_send![&**r, receiverId] };
                string(id.as_deref()) == receiver_id
            }) else {
                return;
            };
            let Some(track) = receiver_track(&receiver) else {
                return;
            };
            match kind(&track).as_str() {
                "audio" => {
                    set_enabled(&track, rtc.speaker_on);
                    record_remote(this, &track);
                }
                "video" => {
                    let media = &mut this.ui.call.media;
                    let slot = FrameSlot::new(false, false, Window::Call, media.tx.clone());
                    let feed = VideoFeed::attach(track, slot);
                    if media.remote_video.is_none() {
                        media.remote_video = Some(feed.clone());
                    }
                    if let Some(mid) = mid {
                        media.remote_videos.insert(mid, feed);
                    }
                }
                _ => {}
            }
        }
        RtcMsg::Statistics(sample) => this.ui.call.media.diagnostics.merge(&sample),
        RtcMsg::LocalLevel(level) => {
            let speaking = level > 0.05 && rtc.audio_track.as_deref().is_some_and(is_enabled);
            this.ui.call.media.local_speaking = speaking;
        }
        RtcMsg::CameraFailed => this.ui.call.media.diagnostics.camera_failed = true,
        RtcMsg::ScreenCaptureError => {
            set_screen_share(this, call_id.clone(), false, cx);
            emit(
                this,
                MediaEvent::ScreenShareStopped {
                    call_id,
                    reason: Some(tr("ui.media_engine.screen_sharing_stopped")),
                },
            );
        }
    }
    cx.notify();
}

fn connection_changed(
    this: &mut Oxide,
    generation: u64,
    call_id: String,
    state: isize,
    cx: &mut Context<Oxide>,
) {
    trace(&format!("connection state={state}"));
    let media = &mut this.ui.call.media;
    let Some((rtc, _)) = current(media, generation) else {
        return;
    };
    match state {
        CONNECTION_CONNECTED => {
            rtc.connection_timeout = None;
            if media.reconnecting {
                trace("reconnected");
            }
            let Some((rtc, _)) = current(media, generation) else {
                return;
            };
            rtc.reconnection = None;
            media.reconnecting = false;
            media.diagnostics.connected = true;
            poll_statistics(this, generation, cx);
            poll_speakers(this, generation, cx);
            emit(this, MediaEvent::Connected { call_id });
        }
        CONNECTION_DISCONNECTED => media.diagnostics.disconnects += 1,
        CONNECTION_FAILED if media.diagnostics.connected => reconnect(this, generation, cx),
        CONNECTION_FAILED => {
            media.diagnostics.failed = true;
            fail(
                this,
                &call_id,
                tr("ui.media_engine.the_audio_video_connection_failed_network"),
            );
        }
        _ => {}
    }
}

// MARK: Statistics and speakers

/// Samples WebRTC statistics every 2 s while connected (energy, packets, RTT, frames).
fn poll_statistics(this: &mut Oxide, generation: u64, cx: &mut Context<Oxide>) {
    let Some((rtc, _)) = current(&mut this.ui.call.media, generation) else {
        return;
    };
    if rtc.stats_task.is_some() {
        return;
    }
    rtc.stats_task = Some(cx.spawn(async move |this, cx| {
        loop {
            cx.background_executor().timer(Duration::from_secs(2)).await;
            let alive = this.update(cx, |this, _| {
                let Some((rtc, _)) = current(&mut this.ui.call.media, generation) else {
                    return false;
                };
                let Some(connection) = rtc.connection() else {
                    return false;
                };
                let post = rtc.post.clone();
                statistics(connection, move |report| {
                    post.send(RtcMsg::Statistics(sample(report)))
                });
                true
            });
            if !matches!(alive, Ok(true)) {
                return;
            }
        }
    }));
}

fn statistics(connection: &AnyObject, done: impl Fn(&AnyObject) + 'static) {
    let block = RcBlock::new(move |report: *mut AnyObject| {
        if let Some(report) = unsafe { report.as_ref() } {
            done(report);
        }
    });
    unsafe {
        let _: () = msg_send![connection, statisticsWithCompletionHandler: &*block];
    }
}

/// Every statistic of a report, as (type, values).
fn statistic_values(report: &AnyObject) -> Vec<(String, Obj)> {
    let all: Option<Obj> = unsafe { msg_send![report, statistics] };
    let Some(all) = all else { return Vec::new() };
    let values: Option<Obj> = unsafe { msg_send![&*all, allValues] };
    media_objc::array(values.as_deref())
        .into_iter()
        .filter_map(|stat| {
            let kind: Option<Obj> = unsafe { msg_send![&*stat, type] };
            let values: Option<Obj> = unsafe { msg_send![&*stat, values] };
            Some((string(kind.as_deref()), values?))
        })
        .collect()
}

fn sample(report: &AnyObject) -> Diagnostics {
    let mut sample = Diagnostics::default();
    let number =
        |values: &AnyObject, key: &str| media_objc::double(media_objc::get(values, key).as_deref());
    for (kind, values) in statistic_values(report) {
        let media_kind = string(media_objc::get(&values, "kind").as_deref());
        match (kind.as_str(), media_kind.as_str()) {
            ("media-source", "audio") => {
                sample.mic_energy = sample.mic_energy.max(number(&values, "totalAudioEnergy"))
            }
            ("inbound-rtp", "audio") => {
                sample.audio_packets_received += number(&values, "packetsReceived") as i64;
                sample.audio_packets_lost += number(&values, "packetsLost").max(0.) as i64;
            }
            ("remote-inbound-rtp", _) => {
                sample.round_trip = sample.round_trip.max(number(&values, "roundTripTime"))
            }
            ("outbound-rtp", "video") => {
                sample.video_frames_sent += number(&values, "framesEncoded") as i64
            }
            _ => {}
        }
    }
    sample
}

/// Every 250 ms: the CSRCs the mixer currently mixes in, and our own input level.
fn poll_speakers(this: &mut Oxide, generation: u64, cx: &mut Context<Oxide>) {
    let Some((rtc, _)) = current(&mut this.ui.call.media, generation) else {
        return;
    };
    if rtc.speaking_task.is_some() {
        return;
    }
    rtc.speaking_task = Some(cx.spawn(async move |this, cx| {
        loop {
            cx.background_executor()
                .timer(Duration::from_millis(250))
                .await;
            let alive = this.update(cx, |this, cx| speakers_tick(this, generation, cx));
            if !matches!(alive, Ok(true)) {
                return;
            }
        }
    }));
}

fn speakers_tick(this: &mut Oxide, generation: u64, cx: &mut Context<Oxide>) -> bool {
    let media = &mut this.ui.call.media;
    let Some((rtc, _)) = current(media, generation) else {
        return false;
    };
    let Some(connection) = rtc.connection.clone() else {
        return false;
    };
    let sources: Vec<Obj> = receivers(&connection)
        .iter()
        .filter(|r| receiver_track(r).is_some_and(|t| kind(&t) == "audio"))
        .flat_map(|r| {
            let list: Option<Obj> = unsafe { msg_send![&**r, sources] };
            media_objc::array(list.as_deref())
        })
        .collect();
    // libwebrtc keeps a source 10 s after its last packet: only a moving timestamp means it is mixed in now.
    let now = Instant::now();
    for source in &sources {
        let kind: isize = unsafe { msg_send![&**source, sourceType] };
        if kind != SOURCE_CSRC {
            continue;
        }
        let id: u32 = unsafe { msg_send![&**source, sourceId] };
        let stamp: f64 = unsafe { msg_send![&**source, timestampUs] };
        let level: Option<Obj> = unsafe { msg_send![&**source, audioLevel] };
        let level = level.as_deref().map_or(1., |l| media_objc::double(Some(l)));
        let previous = rtc.source_stamps.insert(id, stamp);
        if previous.is_some_and(|p| p != stamp) && level > 0.01 {
            rtc.last_heard.insert(id, now);
        }
    }
    rtc.last_heard
        .retain(|_, heard| now.duration_since(*heard) < Duration::from_millis(800));
    let speaking: std::collections::HashSet<u32> = rtc.last_heard.keys().copied().collect();
    let post = rtc.post.clone();
    if speaking != media.speaking_sources {
        let mut sorted: Vec<u32> = speaking.iter().copied().collect();
        sorted.sort();
        trace(&format!("speaking CSRCs={sorted:?} of {}", sources.len()));
        media.speaking_sources = speaking;
        crate::call_window::redraw(this, cx);
    }
    statistics(&connection, move |report| {
        let level = statistic_values(report)
            .into_iter()
            .find(|(kind, values)| {
                kind == "media-source"
                    && string(media_objc::get(values, "kind").as_deref()) == "audio"
            })
            .map_or(0., |(_, values)| {
                media_objc::double(media_objc::get(&values, "audioLevel").as_deref())
            });
        post.send(RtcMsg::LocalLevel(level));
    });
    true
}

// MARK: Audio

pub fn set_muted(this: &mut Oxide, call_id: &str, muted: bool) {
    let media = &mut this.ui.call.media;
    if media.call_id.as_deref() != Some(call_id) {
        return;
    }
    let Some(rtc) = media.rtc.as_ref() else {
        return;
    };
    if let Some(track) = rtc.audio_track.as_deref() {
        set_enabled(track, !muted);
    }
    rtc.microphone_tap.set_muted(muted);
}

pub fn set_speaker(this: &mut Oxide, call_id: &str, on: bool) {
    let media = &mut this.ui.call.media;
    if media.call_id.as_deref() != Some(call_id) {
        return;
    }
    let Some(rtc) = media.rtc.as_mut() else {
        return;
    };
    rtc.speaker_on = on;
    if let Some(connection) = rtc.connection() {
        for track in audio_tracks(connection) {
            set_enabled(&track, on);
        }
    }
}

/// Points the recording's inputs at the microphone (after processing) and every participants' track.
pub fn record_into(media: &mut Media, recorder: &LocalRecorder) {
    let Some(rtc) = media.rtc.as_mut() else {
        return;
    };
    let muted = !rtc.audio_track.as_deref().is_some_and(is_enabled);
    rtc.microphone_tap.set_muted(muted);
    rtc.microphone_tap.set_input(Some(recorder.make_input()));
    let tracks = rtc.connection().map(audio_tracks).unwrap_or_default();
    for track in tracks {
        add_remote_tap(rtc, recorder, &track);
    }
}

/// Adds a participants' track to the recording in progress.
fn record_remote(this: &mut Oxide, track: &AnyObject) {
    let media = &mut this.ui.call.media;
    let (Some(rtc), Some(recorder)) = (media.rtc.as_mut(), media.recorder.as_ref()) else {
        return;
    };
    add_remote_tap(rtc, recorder, track);
}

fn add_remote_tap(rtc: &mut Rtc, recorder: &LocalRecorder, track: &AnyObject) {
    if rtc
        .remote_taps
        .iter()
        .any(|(t, _)| media_objc::same(t, track))
    {
        return;
    }
    let tap = RemoteAudioTap::new(recorder.make_input());
    unsafe {
        let _: () = msg_send![track, addRenderer: &*tap];
    }
    rtc.remote_taps.push((
        unsafe { Retained::retain(track as *const _ as *mut AnyObject) }.unwrap(),
        tap,
    ));
}

pub fn stop_recording_taps(media: &mut Media) {
    let Some(rtc) = media.rtc.as_mut() else {
        return;
    };
    rtc.microphone_tap.set_input(None);
    for (track, tap) in rtc.remote_taps.drain(..) {
        unsafe {
            let _: () = msg_send![&*track, removeRenderer: &*tap];
        }
    }
}

/// Microphone and output chosen in Settings, matched by CoreAudio UID then by name.
fn apply_audio_devices(rtc: &Rtc, prefs: &crate::prefs::Prefs) {
    let module: Option<Obj> = unsafe { msg_send![&*rtc.factory, audioDeviceModule] };
    let Some(module) = module else { return };
    let pick = |id: Option<u32>, list: Option<Obj>| -> Option<Obj> {
        let id = id?;
        let uid = media_devices::core_audio::uid(id)?;
        let name = media_devices::core_audio::name(id);
        media_objc::array(list.as_deref())
            .into_iter()
            .find(|device| {
                let device_id: Option<Obj> = unsafe { msg_send![&**device, deviceId] };
                let device_name: Option<Obj> = unsafe { msg_send![&**device, name] };
                string(device_id.as_deref()) == uid || Some(string(device_name.as_deref())) == name
            })
    };
    let inputs: Option<Obj> = unsafe { msg_send![&*module, inputDevices] };
    if let Some(device) = pick(media_devices::resolved_microphone(prefs), inputs) {
        let _: bool = unsafe { msg_send![&*module, trySetInputDevice: &*device] };
    }
    let outputs: Option<Obj> = unsafe { msg_send![&*module, outputDevices] };
    if let Some(device) = pick(media_devices::resolved_speaker(prefs), outputs) {
        let _: bool = unsafe { msg_send![&*module, trySetOutputDevice: &*device] };
    }
}

// MARK: Video

#[repr(C)]
#[derive(Clone, Copy)]
struct CMVideoDimensions {
    width: i32,
    height: i32,
}
unsafe extern "C" {
    fn CMVideoFormatDescriptionGetDimensions(
        description: *mut CMFormatDescription,
    ) -> CMVideoDimensions;
}

fn format_width(format: &AnyObject) -> i32 {
    let description: *mut CMFormatDescription = unsafe { msg_send![format, formatDescription] };
    if description.is_null() {
        return 0;
    }
    unsafe { CMVideoFormatDescriptionGetDimensions(description) }.width
}

/// 720p when available, else the largest format up to 1280 px wide.
fn camera_format(device: &AnyObject) -> Option<Obj> {
    let cls = class(c"LKRTCCameraVideoCapturer")?;
    let formats: Option<Obj> = unsafe { msg_send![cls, supportedFormatsForDevice: device] };
    let formats = media_objc::array(formats.as_deref());
    if let Some(format) = formats.iter().find(|f| format_width(f) == 1280) {
        return Some(format.clone());
    }
    formats
        .iter()
        .filter(|f| format_width(f) <= 1280)
        .max_by_key(|f| format_width(f))
        .cloned()
        .or_else(|| formats.first().cloned())
}

/// The camera chosen in Settings, capturing; its feed becomes `local_video`.
fn make_camera_track(rtc: &mut Rtc, media: &mut Media, prefs: &crate::prefs::Prefs) -> Option<Obj> {
    let device = media_devices::resolved_camera(prefs)?;
    let source: Obj = unsafe { msg_send![&*rtc.factory, videoSource] };
    let capturer: Obj = unsafe {
        msg_send![media_objc::alloc(c"LKRTCCameraVideoCapturer")?, initWithDelegate: &*source]
    };
    if let Some(format) = camera_format(&device) {
        let ranges: Option<Obj> = unsafe { msg_send![&*format, videoSupportedFrameRateRanges] };
        let max = media_objc::array(ranges.as_deref())
            .iter()
            .map(|r| unsafe { msg_send![&**r, maxFrameRate] })
            .fold(0f64, f64::max);
        let fps = if max > 0. { max.min(30.) as isize } else { 30 };
        let post = rtc.post.clone();
        let done = RcBlock::new(move |error: *mut AnyObject| {
            if !error.is_null() {
                post.send(RtcMsg::CameraFailed);
            }
        });
        unsafe {
            let _: () = msg_send![
                &*capturer,
                startCaptureWithDevice: &*device,
                format: &*format,
                fps: fps,
                completionHandler: &*done
            ];
        }
    }
    rtc.capturer = Some(capturer);
    let track: Obj = unsafe {
        msg_send![&*rtc.factory, videoTrackWithSource: &*source, trackId: &*ns("video0")]
    };
    let slot = FrameSlot::new(true, true, Window::Call, media.tx.clone());
    media.local_video = Some(VideoFeed::attach(track.clone(), slot));
    rtc.camera_track = Some(track.clone());
    Some(track)
}

fn stop_camera(rtc: &Rtc) {
    if let Some(capturer) = rtc.capturer.as_deref() {
        unsafe {
            let _: () = msg_send![capturer, stopCapture];
        }
    }
}

/// Mid-call, the camera line changes direction and renegotiates, like the web client's `startRenegotiationAsync`.
pub fn set_camera(this: &mut Oxide, call_id: String, on: bool, cx: &mut Context<Oxide>) {
    let media = &mut this.ui.call.media;
    if media.call_id.as_deref() != Some(&call_id) {
        return;
    }
    let Some(rtc) = media.rtc.as_ref() else {
        return;
    };
    let Some(connection) = rtc.connection() else {
        return;
    };
    // Before the first answer, or with a negotiation in flight: only the track changes.
    let remote: Option<Obj> = unsafe { msg_send![connection, remoteDescription] };
    if remote.is_none() || signaling_state(connection) != SIGNALING_STABLE {
        if let Some(track) = rtc.camera_track.as_deref() {
            set_enabled(track, on);
        }
        return;
    }
    let generation = rtc.post.generation;
    cx.spawn(async move |this, cx| {
        if on && !permissions::request(MediaKind::Video).await {
            let _ = this.update(cx, |this, _| {
                this.ui.call.media.diagnostics.camera_permission_denied = true
            });
            return;
        }
        let _ = this.update(cx, |this, cx| {
            let prefs = this.prefs.clone();
            let media = &mut this.ui.call.media;
            let Some(mut rtc) = media.rtc.take_if(|r| r.post.generation == generation) else {
                return;
            };
            let line = video_line(&rtc, false);
            if on && line.is_some() && rtc.camera_track.is_none() {
                make_camera_track(&mut rtc, media, &prefs);
            }
            media.rtc = Some(rtc);
            let Some(rtc) = media.rtc.as_mut() else {
                return;
            };
            let Some(line) = line else { return };
            if on {
                let Some(track) = rtc.camera_track.clone() else {
                    return;
                };
                set_enabled(&track, true);
                set_sender_track(&line, Some(&track));
                set_direction(&line, DIRECTION_SEND_RECV);
            } else {
                stop_camera(rtc);
                set_sender_track(&line, None);
                set_direction(&line, DIRECTION_RECV_ONLY);
                rtc.camera_track = None;
                rtc.capturer = None;
                media.local_video = None;
            }
            let Some(rtc) = media.rtc.as_mut() else {
                return;
            };
            rtc.camera_transceiver = Some(line);
            renegotiate(rtc, if on { "camera on" } else { "camera off" });
            cx.notify();
        });
    })
    .detach();
}

/// The remembered video line, else by position (an incoming call's lines come from the caller's offer).
fn video_line(rtc: &Rtc, sharing: bool) -> Option<Obj> {
    let remembered = if sharing {
        &rtc.sharing_transceiver
    } else {
        &rtc.camera_transceiver
    };
    if let Some(line) = remembered.as_ref().filter(|l| !is_stopped(l)) {
        return Some(line.clone());
    }
    let lines: Vec<Obj> = transceivers(rtc.connection()?)
        .into_iter()
        .filter(|t| {
            let kind: isize = unsafe { msg_send![&**t, mediaType] };
            kind == MEDIA_VIDEO && !is_stopped(t)
        })
        .collect();
    if lines.len() <= usize::from(sharing) {
        return None;
    }
    if sharing {
        lines.last().cloned()
    } else {
        lines.first().cloned()
    }
}

/// A local track's frame size changed (a shared window can resize): its encoding follows.
pub fn frame_size(this: &mut Oxide, slot: u64, width: i32, height: i32, cx: &mut Context<Oxide>) {
    let media = &mut this.ui.call.media;
    let camera = media
        .local_video
        .as_ref()
        .filter(|f| f.slot.id == slot)
        .map(|f| f.track_id());
    let Some(rtc) = media.rtc.as_mut() else {
        return;
    };
    let screen = rtc
        .screen_probe
        .as_ref()
        .filter(|f| f.slot.id == slot)
        .map(|f| f.track_id());
    let Some(id) = camera.or(screen) else { return };
    rtc.frame_sizes.insert(id.clone(), (width, height));
    reapply_send_limits(rtc, &id, cx);
}

fn line_for_mid(rtc: &Rtc, mid: &str) -> Option<Obj> {
    transceivers(rtc.connection()?)
        .into_iter()
        .find(|t| self::mid(t) == mid && !is_stopped(t))
}

fn reapply_send_limits(rtc: &mut Rtc, id: &str, cx: &mut Context<Oxide>) {
    let Some(connection) = rtc.connection() else {
        return;
    };
    let Some(line) = transceivers(connection)
        .into_iter()
        .find(|t| sender_track(t).is_some_and(|track| track_id(&track) == id))
    else {
        return;
    };
    let line_mid = mid(&line);
    if let Some(limits) = rtc.send_limits.get(&line_mid).cloned() {
        apply(rtc, &limits, &line, false, cx);
    } else if rtc
        .screen_track
        .as_deref()
        .is_some_and(|t| track_id(t) == id)
    {
        // Before the mixer says anything, the sharing line's own bounds (`applySenderParamsBeforeStart`).
        let limits = SendLimits {
            mid: line_mid,
            sharing: true,
            max_fs: 8160,
            max_fps: 15.,
            max_bitrate: 0,
            keyframe: true,
        };
        apply(rtc, &limits, &line, true, cx);
    }
}

pub fn apply_send_limits(
    this: &mut Oxide,
    call_id: &str,
    limits: SendLimits,
    cx: &mut Context<Oxide>,
) {
    let media = &mut this.ui.call.media;
    if media.call_id.as_deref() != Some(call_id) {
        return;
    }
    let Some(rtc) = media.rtc.as_mut() else {
        return;
    };
    rtc.send_limits.insert(limits.mid.clone(), limits.clone());
    let Some(line) = line_for_mid(rtc, &limits.mid) else {
        return;
    };
    if sender_track(&line).is_none() {
        return;
    }
    apply(rtc, &limits, &line, limits.keyframe, cx);
}

fn set_encodings(line: &AnyObject, scale: f64, fps: f64, max_bitrate: u32) {
    let Some(sender) = sender(line) else { return };
    let parameters: Option<Obj> = unsafe { msg_send![&*sender, parameters] };
    let Some(parameters) = parameters else { return };
    let encodings: Option<Obj> = unsafe { msg_send![&*parameters, encodings] };
    for encoding in media_objc::array(encodings.as_deref()) {
        unsafe {
            let _: () =
                msg_send![&*encoding, setScaleResolutionDownBy: &*media_objc::number(scale)];
            let _: () = msg_send![&*encoding, setMaxFramerate: &*media_objc::number(fps)];
            if max_bitrate > 0 {
                let _: () = msg_send![&*encoding, setMaxBitrateBps: &*media_objc::number(max_bitrate as f64)];
            }
        }
    }
    unsafe {
        let _: () = msg_send![&*sender, setParameters: &*parameters];
    }
}

/// Web client's `updateEncodingsWithCaps` / `generateKeyFrameIfNeeded`: screens capped at 8160 fs and 15 fps;
/// keyframe via a brief 10 % upscale.
fn apply(rtc: &Rtc, limits: &SendLimits, line: &Obj, keyframe: bool, cx: &mut Context<Oxide>) {
    let Some(track) = sender_track(line) else {
        return;
    };
    let Some(&(width, height)) = rtc.frame_sizes.get(&track_id(&track)) else {
        return;
    };
    if width <= 0 {
        return;
    }
    let max_fs = if limits.sharing {
        limits.max_fs.min(8160)
    } else {
        limits.max_fs
    } as f64;
    let fps = if limits.sharing {
        limits.max_fps.min(15.)
    } else {
        limits.max_fps
    };
    let scale = (width as f64 * height as f64 / (max_fs * 256.))
        .sqrt()
        .max(1.);
    trace(&format!(
        "send limits mid={} {width}x{height} fs={} scale={scale:.2} fps={fps} br={} keyframe={keyframe}",
        mid(line),
        max_fs as i64,
        limits.max_bitrate
    ));
    if !keyframe {
        set_encodings(line, scale, fps, limits.max_bitrate);
        return;
    }
    set_encodings(line, scale * 1.1, fps, limits.max_bitrate);
    let generation = rtc.post.generation;
    let line = line.clone();
    let max_bitrate = limits.max_bitrate;
    cx.spawn(async move |this, cx| {
        cx.background_executor()
            .timer(Duration::from_millis(100))
            .await;
        let _ = this.update(cx, |this, _| {
            if current(&mut this.ui.call.media, generation).is_some() && !is_stopped(&line) {
                set_encodings(&line, scale, fps, max_bitrate);
            }
        });
    })
    .detach();
}

// MARK: Screen share

/// Drops the strong `self` libwebrtc's desktop capturer/list native half keeps (else never freed, ~60 MB);
/// once only (webrtc-xcframework 144.7559.15).
pub fn release_native_self_reference(object: &AnyObject) {
    unsafe { objc2::ffi::objc_release(object as *const AnyObject as *mut AnyObject) };
}

/// Captures the chosen screen (else the main screen) on the sharing line, or stops; renegotiates either way.
pub fn set_screen_share(this: &mut Oxide, call_id: String, on: bool, _cx: &mut Context<Oxide>) {
    let media = &mut this.ui.call.media;
    let ready = media.call_id.as_deref() == Some(&call_id)
        && media
            .rtc
            .as_ref()
            .and_then(Rtc::connection)
            .is_some_and(|c| signaling_state(c) == SIGNALING_STABLE);
    if !ready {
        if on {
            emit(
                this,
                MediaEvent::ScreenShareStopped {
                    call_id,
                    reason: Some(tr("ui.media_engine.the_media_connection_isn_t_ready")),
                },
            );
        }
        return;
    }
    if on && !permissions::screen_capture(true) {
        emit(
            this,
            MediaEvent::ScreenShareStopped {
                call_id,
                reason: Some(tr("ui.media_engine.allow_oxide_in_system_settings_privacy")),
            },
        );
        return;
    }
    let chosen = media.chosen_screen.take();
    let tx = media.tx.clone();
    let Some(rtc) = media.rtc.as_mut() else {
        return;
    };
    let Some(connection) = rtc.connection.clone() else {
        return;
    };
    if on {
        stop_screen_capture(rtc);
        let source: Obj = unsafe { msg_send![&*rtc.factory, videoSourceForScreenCast: true] };
        let capturer: Option<Obj> = unsafe {
            match &chosen {
                Some(choice) => msg_send![
                    media_objc::alloc(c"LKRTCDesktopCapturer").unwrap(),
                    initWithSource: &*choice.source,
                    delegate: &*rtc.capture_delegate,
                    captureDelegate: &*source
                ],
                None => msg_send![
                    media_objc::alloc(c"LKRTCDesktopCapturer").unwrap(),
                    initWithDefaultScreen: &*rtc.capture_delegate,
                    captureDelegate: &*source
                ],
            }
        };
        // The capture has read what it needs from the lists.
        if let Some(choice) = chosen {
            choice.lists.close();
        }
        let Some(capturer) = capturer else { return };
        unsafe {
            let _: () = msg_send![&*capturer, startCaptureWithFPS: 15isize];
        }
        rtc.screen_capturer = Some(capturer);
        let track: Obj = unsafe {
            msg_send![&*rtc.factory, videoTrackWithSource: &*source, trackId: &*ns("screen0")]
        };
        rtc.screen_probe = Some(VideoFeed::attach(
            track.clone(),
            FrameSlot::new(false, true, Window::Call, tx),
        ));
        rtc.screen_track = Some(track.clone());
        // Send-only like the web client: offered both ways, the mixer answers `sendonly` and never takes our screen.
        if let Some(line) = video_line(rtc, true) {
            set_sender_track(&line, Some(&track));
            set_direction(&line, DIRECTION_SEND_ONLY);
            rtc.sharing_transceiver = Some(line);
        } else if let Some(options) = media_objc::new(c"LKRTCRtpTransceiverInit") {
            let streams = media_objc::nsarray_of_strings(&["local".to_owned()]);
            unsafe {
                let _: () = msg_send![&*options, setDirection: DIRECTION_SEND_ONLY];
                let _: () = msg_send![&*options, setStreamIds: &*streams];
                rtc.sharing_transceiver =
                    msg_send![&*connection, addTransceiverWithTrack: &*track, init: &*options];
            }
        }
    } else {
        stop_screen_capture(rtc);
        let Some(line) = video_line(rtc, true) else {
            return;
        };
        set_sender_track(&line, None);
        set_direction(&line, DIRECTION_RECV_ONLY);
    }
    renegotiate(
        rtc,
        if on {
            "screen share on"
        } else {
            "screen share off"
        },
    );
}

fn stop_screen_capture_of(rtc: &Rtc) {
    if let Some(capturer) = rtc.screen_capturer.as_deref() {
        unsafe {
            let _: () = msg_send![capturer, stopCapture];
        }
        release_native_self_reference(capturer);
    }
}

fn stop_screen_capture(rtc: &mut Rtc) {
    stop_screen_capture_of(rtc);
    rtc.screen_capturer = None;
    rtc.screen_probe = None;
    rtc.screen_track = None;
}

// MARK: Delegates

define_class!(
    // SAFETY: NSObject has no subclassing requirements; the ivars are thread-safe.
    #[unsafe(super(NSObject))]
    #[name = "OxidePeerDelegate"]
    #[ivars = Post]
    struct PeerDelegate;

    impl PeerDelegate {
        #[unsafe(method(peerConnection:didChangeSignalingState:))]
        fn signaling(&self, _connection: *mut AnyObject, _state: isize) {}

        #[unsafe(method(peerConnection:didAddStream:))]
        fn add_stream(&self, _connection: *mut AnyObject, _stream: *mut AnyObject) {}

        #[unsafe(method(peerConnection:didRemoveStream:))]
        fn remove_stream(&self, _connection: *mut AnyObject, _stream: *mut AnyObject) {}

        #[unsafe(method(peerConnectionShouldNegotiate:))]
        fn should_negotiate(&self, _connection: *mut AnyObject) {}

        #[unsafe(method(peerConnection:didChangeIceConnectionState:))]
        fn ice_connection(&self, _connection: *mut AnyObject, _state: isize) {}

        #[unsafe(method(peerConnection:didChangeIceGatheringState:))]
        fn gathering(&self, _connection: *mut AnyObject, state: isize) {
            if state == GATHERING_COMPLETE {
                self.ivars().send(RtcMsg::GatheringComplete);
            }
        }

        #[unsafe(method(peerConnection:didGenerateIceCandidate:))]
        fn candidate(&self, _connection: *mut AnyObject, _candidate: *mut AnyObject) {}

        #[unsafe(method(peerConnection:didRemoveIceCandidates:))]
        fn remove_candidates(&self, _connection: *mut AnyObject, _candidates: *mut AnyObject) {}

        #[unsafe(method(peerConnection:didOpenDataChannel:))]
        fn data_channel(&self, _connection: *mut AnyObject, _channel: *mut AnyObject) {}

        #[unsafe(method(peerConnection:didChangeConnectionState:))]
        fn connection_state(&self, _connection: *mut AnyObject, state: isize) {
            trace(&format!("connection state={state}"));
            self.ivars().send(RtcMsg::ConnectionState(state));
        }

        #[unsafe(method(peerConnection:didAddReceiver:streams:))]
        fn add_receiver(&self, connection: *mut AnyObject, receiver: *mut AnyObject, _streams: *mut AnyObject) {
            let (Some(connection), Some(receiver)) = (unsafe { connection.as_ref() }, unsafe { receiver.as_ref() })
            else {
                return;
            };
            let id: Option<Obj> = unsafe { msg_send![receiver, receiverId] };
            let receiver_id = string(id.as_deref());
            let mid = transceivers(connection)
                .into_iter()
                .find(|t| {
                    let r: Option<Obj> = unsafe { msg_send![&**t, receiver] };
                    r.is_some_and(|r| {
                        let rid: Option<Obj> = unsafe { msg_send![&*r, receiverId] };
                        string(rid.as_deref()) == receiver_id
                    })
                })
                .map(|t| mid(&t));
            self.ivars().send(RtcMsg::AddedReceiver { receiver_id, mid });
        }
    }
);

impl PeerDelegate {
    fn new(post: Post) -> Retained<Self> {
        let this = Self::alloc().set_ivars(post);
        unsafe { msg_send![super(this), init] }
    }
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements; the ivars are thread-safe.
    #[unsafe(super(NSObject))]
    #[name = "OxideDesktopCaptureDelegate"]
    #[ivars = Post]
    struct CaptureDelegate;

    impl CaptureDelegate {
        #[unsafe(method(didSourceCaptureStart:))]
        fn started(&self, _capturer: *mut AnyObject) {}

        #[unsafe(method(didSourceCapturePaused:))]
        fn paused(&self, _capturer: *mut AnyObject) {}

        #[unsafe(method(didSourceCaptureStop:))]
        fn stopped(&self, _capturer: *mut AnyObject) {}

        #[unsafe(method(didSourceCaptureError:))]
        fn failed(&self, _capturer: *mut AnyObject) {
            self.ivars().send(RtcMsg::ScreenCaptureError);
        }
    }
);

impl CaptureDelegate {
    fn new(post: Post) -> Retained<Self> {
        let this = Self::alloc().set_ivars(post);
        unsafe { msg_send![super(this), init] }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real offer from LiveKit's libwebrtc, without capturing anything: binding, factory, transceivers and the
    /// completion blocks all work. Skipped when the framework is missing.
    #[test]
    fn offers_audio_and_video_lines() {
        if !media_objc::available() {
            eprintln!("LiveKitWebRTC not linked: skipped");
            return;
        }
        let (tx, mut rx) = futures::channel::mpsc::unbounded();
        let rtc = Rtc::new(Post { tx, generation: 7 }).expect("factory");
        let connection = make_connection(&rtc, &[]).expect("peer connection");
        add_receive_video(&connection).expect("video line");
        add_receive_video(&connection).expect("sharing line");
        describe(&connection, true, Purpose::Start, &rtc.post);
        let deadline = Instant::now() + Duration::from_secs(10);
        let sdp = loop {
            assert!(Instant::now() < deadline, "no offer");
            match rx.try_recv() {
                Ok(MediaMsg::Rtc(7, RtcMsg::Described { result, .. })) => {
                    break result.expect("offer").1;
                }
                _ => std::thread::sleep(Duration::from_millis(20)),
            }
        };
        assert_eq!(sdp.matches("m=video").count(), 2);
        assert_eq!(transceivers(&connection).len(), 2);
        unsafe {
            let _: () = msg_send![&*connection, close];
        }
    }
}

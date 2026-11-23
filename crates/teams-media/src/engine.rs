//! The call's peer connection, run by one actor: app commands and libwebrtc callbacks share its queue, so no lock.
use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;
use std::time::{Duration, Instant};

use libwebrtc::MediaType;
use libwebrtc::media_stream_track::MediaStreamTrack;
use libwebrtc::peer_connection::{
    AnswerOptions, IceGatheringState, OfferOptions, PeerConnection, PeerConnectionState,
    SignalingState,
};
use libwebrtc::peer_connection_factory::native::PeerConnectionFactoryExt;
use libwebrtc::peer_connection_factory::{
    ContinualGatheringPolicy, IceServer as RtcIceServer, PeerConnectionFactory, RtcConfiguration,
};
use libwebrtc::prelude::{RtcAudioTrack, RtcVideoTrack};
use libwebrtc::rtp_parameters::RtpEncodingParameters;
use libwebrtc::rtp_transceiver::{RtpTransceiver, RtpTransceiverDirection, RtpTransceiverInit};
use libwebrtc::session_description::{SdpType, SessionDescription};
use tokio::sync::mpsc;

use crate::camera::Camera;
use crate::screen::{ScreenCapture, ScreenSource};
use crate::stats::{CallDiagnostics, sample};
use crate::video::{Sinks, VideoSink, VideoSlot, pump};
use crate::{
    Devices, IceServer, Listener, MediaEvent, MediaListener, PcmSink, PrepareOptions, SendLimits,
    text,
};

/// How long to wait for ICE gathering before sending what was gathered.
const GATHERING_LIMIT: Duration = Duration::from_secs(3);
/// From the answer to a connected transport.
const CONNECT_LIMIT: Duration = Duration::from_secs(30);
/// The web client's `reconnectTimeLimit`: how long a call that was connected tries to get its media back.
const RECONNECT_LIMIT: Duration = Duration::from_secs(90);
/// The web client's screen bounds (`applySenderParamsBeforeStart`).
const SCREEN_MAX_FS: u32 = 8160;
const SCREEN_MAX_FPS: f64 = 15.0;
/// libwebrtc keeps a source 10 s after its last packet; one not heard for this long has stopped talking.
const SPEAKING_HOLD: Duration = Duration::from_millis(800);

/// Handle to the engine thread. Dropping it closes any call.
pub struct MediaEngine {
    tx: mpsc::UnboundedSender<Msg>,
    sinks: Sinks,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl MediaEngine {
    /// An engine on the platform's microphone and speakers.
    pub fn new(listener: Arc<dyn MediaListener>) -> Self {
        Self::start(listener, false)
    }

    /// No device: a tone for a microphone, a test pattern for a camera; for tests.
    pub fn synthetic(listener: Arc<dyn MediaListener>) -> Self {
        Self::start(listener, true)
    }

    fn start(listener: Arc<dyn MediaListener>, synthetic: bool) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let sinks = Sinks::default();
        let actor = Actor::new(listener, tx.clone(), sinks.clone(), synthetic);
        let thread = std::thread::Builder::new()
            .name("teams-media".into())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(2)
                    .thread_name("teams-media-worker")
                    .enable_all()
                    .build()
                    .expect("media runtime");
                runtime.block_on(actor.run(rx));
            })
            .expect("media thread");
        Self {
            tx,
            sinks,
            thread: Some(thread),
        }
    }

    fn send(&self, command: Command) {
        let _ = self.tx.send(Msg::Command(command));
    }

    /// Offers, or answers `remote_offer`; the SDP comes back as [`MediaEvent::LocalDescription`].
    pub fn prepare(&self, options: PrepareOptions) {
        self.send(Command::Prepare(options));
    }
    /// The service's answer (`provisional`: early media, a pranswer).
    pub fn set_remote_description(&self, call_id: &str, sdp: &str, provisional: bool) {
        self.send(Command::RemoteDescription {
            call_id: call_id.into(),
            sdp: sdp.into(),
            provisional,
        });
    }
    /// The service renegotiates; the answer comes back as [`MediaEvent::RenegotiationAnswer`].
    pub fn answer_remote_offer(&self, call_id: &str, sdp: &str) {
        self.send(Command::RemoteOffer {
            call_id: call_id.into(),
            sdp: sdp.into(),
        });
    }
    /// The service rejected our renegotiation: offer again, or go back to what was last negotiated.
    pub fn offer_rejected(&self, call_id: &str, retry: bool) {
        self.send(Command::OfferRejected {
            call_id: call_id.into(),
            retry,
        });
    }
    /// New ICE credentials after a network change; the new offer comes as `LocalDescription`.
    pub fn restart_ice(&self, call_id: &str) {
        self.send(Command::RestartIce {
            call_id: call_id.into(),
        });
    }
    pub fn set_muted(&self, call_id: &str, muted: bool) {
        self.send(Command::Muted {
            call_id: call_id.into(),
            muted,
        });
    }
    /// Plays or silences the other participants on this device.
    pub fn set_speaker(&self, call_id: &str, on: bool) {
        self.send(Command::Speaker {
            call_id: call_id.into(),
            on,
        });
    }
    /// Turns the camera line on or off mid-call; renegotiates.
    pub fn set_camera(&self, call_id: &str, on: bool) {
        self.send(Command::Camera {
            call_id: call_id.into(),
            on,
        });
    }
    /// Shares `source` (else the main screen) on the sharing line, or stops; renegotiates either way.
    pub fn set_screen_share(&self, call_id: &str, on: bool, source: Option<ScreenSource>) {
        self.send(Command::ScreenShare {
            call_id: call_id.into(),
            on,
            source,
        });
    }
    /// The mixer's bounds for one of our sending lines, and a keyframe when it asks.
    pub fn apply_send_limits(&self, call_id: &str, limits: SendLimits) {
        self.send(Command::SendLimits {
            call_id: call_id.into(),
            limits,
        });
    }
    /// Microphone, speaker and camera for this call and the next ones; applied now when in a call.
    pub fn set_devices(&self, devices: Devices) {
        self.send(Command::Devices(devices));
    }
    /// Raw PCM of the microphone (after processing) and of the participants, for a local recording.
    pub fn set_pcm_sink(&self, sink: Option<Arc<dyn PcmSink>>) {
        self.send(Command::Pcm(sink));
    }
    /// Shows `slot`'s frames on `sink`, or stops showing them.
    pub fn set_video_sink(&self, slot: VideoSlot, sink: Option<Arc<dyn VideoSink>>) {
        self.sinks.set(slot, sink);
    }
    /// Stops capture and closes the peer connection.
    pub fn close(&self, call_id: &str) {
        self.send(Command::Close {
            call_id: call_id.into(),
        });
    }
}

impl Drop for MediaEngine {
    fn drop(&mut self) {
        let _ = self.tx.send(Msg::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

enum Command {
    Prepare(PrepareOptions),
    RemoteDescription {
        call_id: String,
        sdp: String,
        provisional: bool,
    },
    RemoteOffer {
        call_id: String,
        sdp: String,
    },
    OfferRejected {
        call_id: String,
        retry: bool,
    },
    RestartIce {
        call_id: String,
    },
    Muted {
        call_id: String,
        muted: bool,
    },
    Speaker {
        call_id: String,
        on: bool,
    },
    Camera {
        call_id: String,
        on: bool,
    },
    ScreenShare {
        call_id: String,
        on: bool,
        source: Option<ScreenSource>,
    },
    SendLimits {
        call_id: String,
        limits: SendLimits,
    },
    Devices(Devices),
    Pcm(Option<Arc<dyn PcmSink>>),
    Close {
        call_id: String,
    },
}

/// Internal messages carry their call's generation, so a late callback can't touch the next call.
enum Msg {
    Command(Command),
    GatheringComplete(u64),
    GatheringTimeout(u64, u64),
    ConnectionState(u64, PeerConnectionState),
    ConnectTimeout(u64, u64),
    Track(u64, Option<String>, MediaStreamTrack),
    ReconnectTick(u64),
    StatsTick(u64),
    SpeakersTick(u64),
    FrameSize(u64, VideoSlot, u32, u32),
    CaptureFailed(u64, VideoSlot),
    Shutdown,
}

/// What `wait_for_gathering` sends once complete.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pending {
    None,
    /// Our offer, or our answer to an incoming call.
    Local,
    /// Our answer to the service's renegotiation.
    RenegotiationAnswer,
}

struct Call {
    id: String,
    generation: u64,
    factory: PeerConnectionFactory,
    pc: PeerConnection,
    audio: RtcAudioTrack,
    muted: bool,
    speaker: bool,
    /// Remote audio tracks, silenced by `speaker`.
    remote_audio: Vec<RtcAudioTrack>,
    camera: Option<Camera>,
    camera_track: Option<RtcVideoTrack>,
    screen: Option<ScreenCapture>,
    screen_track: Option<RtcVideoTrack>,
    /// The lines our camera and our screen go out on, kept from the offer that created them.
    camera_line: Option<RtpTransceiver>,
    sharing_line: Option<RtpTransceiver>,
    pending: Pending,
    /// Bumped on each new local description, so a stale gathering timer doesn't send.
    negotiation: u64,
    connected_once: bool,
    reconnect_deadline: Option<Instant>,
    reconnect_attempts: u32,
    reconnect_next: Instant,
    /// Our camera's and screen's frame size, to scale to the mixer's bounds.
    frame_sizes: HashMap<VideoSlot, (u32, u32)>,
    /// The mixer's last bounds by mid, applied again when a frame size changes.
    send_limits: HashMap<String, SendLimits>,
    pumps: Vec<tokio::task::JoinHandle<()>>,
    /// Participants' audio to the PCM sink, while recording.
    pcm_pumps: Vec<tokio::task::JoinHandle<()>>,
    timers: Vec<tokio::task::JoinHandle<()>>,
    last_heard: HashMap<u32, Instant>,
    /// Each CSRC's packet timestamp at the previous poll: it moves only while that voice is mixed in.
    source_stamps: HashMap<u32, i64>,
    speaking: BTreeSet<u32>,
    local_speaking: bool,
    diagnostics: CallDiagnostics,
    remote_videos: BTreeSet<String>,
}

struct Actor {
    listener: Listener,
    tx: mpsc::UnboundedSender<Msg>,
    sinks: Sinks,
    devices: Devices,
    pcm: Option<Arc<dyn PcmSink>>,
    call: Option<Call>,
    generation: u64,
    /// See [`MediaEngine::synthetic`].
    synthetic: bool,
}

impl Actor {
    fn new(
        listener: Listener,
        tx: mpsc::UnboundedSender<Msg>,
        sinks: Sinks,
        synthetic: bool,
    ) -> Self {
        Self {
            listener,
            tx,
            sinks,
            devices: Devices::default(),
            pcm: None,
            call: None,
            generation: 0,
            synthetic,
        }
    }

    async fn run(mut self, mut rx: mpsc::UnboundedReceiver<Msg>) {
        while let Some(msg) = rx.recv().await {
            match msg {
                Msg::Shutdown => break,
                Msg::Command(command) => self.command(command).await,
                msg => self.internal(msg).await,
            }
        }
        if let Some(call) = self.call.take() {
            self.teardown(call);
        }
    }

    fn emit(&self, event: MediaEvent) {
        self.listener.on_event(event);
    }

    fn call_mut(&mut self, call_id: &str) -> Option<&mut Call> {
        self.call.as_mut().filter(|c| c.id == call_id)
    }

    fn current(&mut self, generation: u64) -> Option<&mut Call> {
        self.call.as_mut().filter(|c| c.generation == generation)
    }

    /// Sends `msg` after `delay`, unless the current call ends first.
    fn schedule(&mut self, delay: Duration, msg: Msg) {
        let tx = self.tx.clone();
        let timer = tokio::spawn(async move {
            tokio::time::sleep(delay).await;
            let _ = tx.send(msg);
        });
        if let Some(call) = self.call.as_mut() {
            call.timers.push(timer);
        }
    }

    fn every(
        &self,
        period: Duration,
        msg: fn(u64) -> Msg,
        generation: u64,
    ) -> tokio::task::JoinHandle<()> {
        let tx = self.tx.clone();
        tokio::spawn(async move {
            let mut ticks = tokio::time::interval(period);
            ticks.tick().await;
            loop {
                ticks.tick().await;
                if tx.send(msg(generation)).is_err() {
                    return;
                }
            }
        })
    }

    async fn command(&mut self, command: Command) {
        match command {
            Command::Prepare(options) => self.prepare(options).await,
            Command::RemoteDescription {
                call_id,
                sdp,
                provisional,
            } => self.remote_description(&call_id, &sdp, provisional).await,
            Command::RemoteOffer { call_id, sdp } => self.remote_offer(&call_id, &sdp).await,
            Command::OfferRejected { call_id, retry } => self.offer_rejected(&call_id, retry).await,
            Command::RestartIce { call_id } => {
                if self.call_mut(&call_id).is_some() {
                    self.renegotiate("ICE restart", true).await;
                }
            }
            Command::Muted { call_id, muted } => {
                let synthetic = self.synthetic;
                let pcm = self.pcm.clone();
                if let Some(call) = self.call_mut(&call_id) {
                    call.muted = muted;
                    call.audio.set_enabled(!muted);
                    // The microphone is still captured while muted: keep it out of the recording.
                    if !synthetic {
                        crate::audio::set_mic_tap(pcm.filter(|_| !muted));
                    }
                }
            }
            Command::Speaker { call_id, on } => {
                if let Some(call) = self.call_mut(&call_id) {
                    call.speaker = on;
                    for track in &call.remote_audio {
                        track.set_enabled(on);
                    }
                }
            }
            Command::Camera { call_id, on } => self.set_camera(&call_id, on).await,
            Command::ScreenShare {
                call_id,
                on,
                source,
            } => self.set_screen_share(&call_id, on, source).await,
            Command::SendLimits { call_id, limits } => {
                if let Some(call) = self.call_mut(&call_id) {
                    apply_limits(call, &limits, limits.keyframe);
                    call.send_limits.insert(limits.mid.clone(), limits);
                }
            }
            Command::Devices(devices) => {
                if let Some(call) = &self.call {
                    crate::devices::apply(
                        &call.factory,
                        devices.microphone.as_ref(),
                        devices.speaker.as_ref(),
                    );
                    restart_audio(&call.factory);
                }
                self.devices = devices;
            }
            Command::Pcm(sink) => {
                self.pcm = sink;
                if let Some(call) = self.call.as_mut() {
                    if !self.synthetic {
                        crate::audio::set_mic_tap(self.pcm.clone().filter(|_| !call.muted));
                    }
                    for task in call.pcm_pumps.drain(..) {
                        task.abort();
                    }
                    if let Some(sink) = &self.pcm {
                        for track in &call.remote_audio {
                            call.pcm_pumps
                                .push(crate::audio::pump_remote(track.clone(), sink.clone()));
                        }
                    }
                }
            }
            Command::Close { call_id } => {
                if let Some(call) = self.call.take_if(|c| c.id == call_id) {
                    self.teardown(call);
                }
            }
        }
    }

    async fn internal(&mut self, msg: Msg) {
        match msg {
            Msg::GatheringComplete(generation) => {
                if self.current(generation).is_some() {
                    self.send_description();
                }
            }
            Msg::GatheringTimeout(generation, negotiation) => {
                if self
                    .current(generation)
                    .is_some_and(|c| c.negotiation == negotiation)
                {
                    self.send_description();
                }
            }
            Msg::ConnectionState(generation, state) => {
                self.connection_state(generation, state).await
            }
            Msg::ConnectTimeout(generation, negotiation) => {
                let Some(call) = self.current(generation) else {
                    return;
                };
                if call.negotiation == negotiation
                    && call.reconnect_deadline.is_none()
                    && call.pc.connection_state() != PeerConnectionState::Connected
                {
                    call.diagnostics.failed = true;
                    self.fail(text("media_negotiation_succeeded_but_the_audio"));
                }
            }
            Msg::Track(generation, mid, track) => {
                let sinks = self.sinks.clone();
                let pcm = self.pcm.clone();
                let Some(call) = self.current(generation) else {
                    return;
                };
                match track {
                    MediaStreamTrack::Audio(audio) => {
                        audio.set_enabled(call.speaker);
                        if let Some(sink) = pcm {
                            call.pcm_pumps
                                .push(crate::audio::pump_remote(audio.clone(), sink));
                        }
                        call.remote_audio.push(audio);
                    }
                    MediaStreamTrack::Video(video) => {
                        let Some(mid) = mid else { return };
                        call.pumps
                            .push(pump(video, VideoSlot::Remote(mid.clone()), sinks, None));
                        if call.remote_videos.insert(mid.clone()) {
                            let id = call.id.clone();
                            self.emit(MediaEvent::Video {
                                call_id: id,
                                slot: VideoSlot::Remote(mid),
                                present: true,
                            });
                        }
                    }
                }
            }
            Msg::ReconnectTick(generation) => self.reconnect_tick(generation).await,
            Msg::StatsTick(generation) => {
                let Some(call) = self.current(generation) else {
                    return;
                };
                let pc = call.pc.clone();
                let Ok(report) = pc.get_stats().await else {
                    return;
                };
                let Some(call) = self.current(generation) else {
                    return;
                };
                let sample = sample(&report);
                call.diagnostics.merge(&sample);
                let video = crate::stats::video_summary(&report);
                if !video.is_empty() {
                    log::info!("video {video}");
                }
                let (id, diagnostics) = (call.id.clone(), call.diagnostics.clone());
                self.emit(MediaEvent::Diagnostics {
                    call_id: id,
                    diagnostics,
                });
            }
            Msg::SpeakersTick(generation) => self.poll_speakers(generation).await,
            Msg::FrameSize(generation, slot, width, height) => {
                let Some(call) = self.current(generation) else {
                    return;
                };
                call.frame_sizes.insert(slot.clone(), (width, height));
                let line = match slot {
                    VideoSlot::Camera => call.camera_line.clone(),
                    VideoSlot::Screen => call.sharing_line.clone(),
                    VideoSlot::Remote(_) => None,
                };
                let Some(mid) = line.and_then(|l| l.mid()) else {
                    return;
                };
                if let Some(limits) = call.send_limits.get(&mid) {
                    apply_limits(call, limits, false);
                } else if slot == VideoSlot::Screen {
                    // Before the mixer says anything, the sharing line's own bounds.
                    let limits = SendLimits {
                        mid,
                        sharing: true,
                        max_fs: SCREEN_MAX_FS,
                        max_fps: SCREEN_MAX_FPS,
                        max_bitrate: 0,
                        keyframe: true,
                    };
                    apply_limits(call, &limits, true);
                }
            }
            Msg::CaptureFailed(generation, slot) => {
                let Some(call) = self.current(generation) else {
                    return;
                };
                let id = call.id.clone();
                match slot {
                    VideoSlot::Camera => call.diagnostics.camera_failed = true,
                    VideoSlot::Screen => {
                        self.set_screen_share(&id, false, None).await;
                        let reason = Some(text("screen_sharing_stopped"));
                        self.emit(MediaEvent::ScreenShareStopped {
                            call_id: id,
                            reason,
                        });
                    }
                    VideoSlot::Remote(_) => {}
                }
            }
            Msg::Command(_) | Msg::Shutdown => unreachable!(),
        }
    }

    async fn prepare(&mut self, options: PrepareOptions) {
        if let Some(call) = self.call.take() {
            self.teardown(call);
        }
        self.generation += 1;
        let generation = self.generation;
        let call_id = options.call_id.clone();
        log::info!(
            "prepare servers={} TURN={} incoming={}",
            options.ice_servers.len(),
            options.ice_servers.iter().any(|s| s.credential.is_some()),
            options.remote_offer.is_some()
        );

        // Per call: libwebrtc's threads, codecs and audio device live only while a call does.
        let factory = PeerConnectionFactory::default();
        if !self.synthetic {
            if !factory.acquire_platform_adm() {
                self.emit_failure(&call_id, text("couldn_t_prepare_the_audio_connection"));
                return;
            }
            factory.set_adm_playout_enabled(true);
            factory.set_adm_recording_enabled(true);
            crate::devices::apply(
                &factory,
                self.devices.microphone.as_ref(),
                self.devices.speaker.as_ref(),
            );
            crate::audio::set_mic_tap(self.pcm.clone().filter(|_| !options.muted));
        }

        let mut config = RtcConfiguration::default();
        config.ice_servers = options.ice_servers.iter().map(rtc_ice_server).collect();
        config.continual_gathering_policy = ContinualGatheringPolicy::GatherOnce;
        let pc = match factory.create_peer_connection(config) {
            Ok(pc) => pc,
            Err(e) => {
                log::warn!("peer connection: {e:?}");
                if !self.synthetic {
                    factory.release_platform_adm();
                }
                self.emit_failure(&call_id, text("couldn_t_prepare_the_audio_connection"));
                return;
            }
        };
        self.observe(&pc, generation);

        let (audio, tone) = if self.synthetic {
            let (track, tone) = crate::audio::tone_track(&factory);
            (track, Some(tone))
        } else {
            (factory.create_device_audio_track("audio0"), None)
        };
        audio.set_enabled(!options.muted);
        let offering = options.remote_offer.is_none();
        let mut call = Call {
            id: call_id.clone(),
            generation,
            factory,
            pc: pc.clone(),
            audio: audio.clone(),
            muted: options.muted,
            speaker: options.speaker,
            remote_audio: Vec::new(),
            camera: None,
            camera_track: None,
            screen: None,
            screen_track: None,
            camera_line: None,
            sharing_line: None,
            pending: Pending::Local,
            negotiation: 0,
            connected_once: false,
            reconnect_deadline: None,
            reconnect_attempts: 0,
            reconnect_next: Instant::now(),
            frame_sizes: HashMap::new(),
            send_limits: HashMap::new(),
            pumps: tone.into_iter().collect(),
            pcm_pumps: Vec::new(),
            timers: Vec::new(),
            last_heard: HashMap::new(),
            source_stamps: HashMap::new(),
            speaking: BTreeSet::new(),
            local_speaking: false,
            diagnostics: CallDiagnostics {
                camera_requested: options.video,
                ..Default::default()
            },
            remote_videos: BTreeSet::new(),
        };
        add_track(&pc, MediaStreamTrack::Audio(audio), offering);
        if options.video {
            if let Some(track) = self.start_camera(&mut call) {
                call.camera_line = add_track(&pc, MediaStreamTrack::Video(track), offering);
            }
        }
        if offering {
            // Like the web client, always offer every camera line and the sharing line: the mixer rejects missing ones.
            let first = u32::from(call.camera_track.is_some());
            for _ in first..options.camera_lines.max(1) {
                let line = add_receive_video(&pc);
                if call.camera_line.is_none() {
                    call.camera_line = line;
                }
            }
            call.sharing_line = add_receive_video(&pc);
        }
        self.call = Some(call);

        let description = match options.remote_offer {
            Some(offer) => {
                if let Err(e) = set_remote(&pc, &offer, SdpType::Offer).await {
                    return self.fail_with("remote_offer_rejected", e);
                }
                pc.create_answer(AnswerOptions::default()).await
            }
            None => pc.create_offer(offer_options(false)).await,
        };
        if self.current(generation).is_none() {
            return;
        }
        let description = match description {
            Ok(description) => description,
            Err(e) => return self.fail_with("couldn_t_create_the_local_media", e.message),
        };
        if let Err(e) = pc.set_local_description(description).await {
            return self.fail_with("local_description_rejected", e.message);
        }
        self.wait_for_gathering(Pending::Local);
    }

    fn observe(&self, pc: &PeerConnection, generation: u64) {
        let tx = self.tx.clone();
        pc.on_ice_gathering_state_change(Some(Box::new(move |state| {
            if state == IceGatheringState::Complete {
                let _ = tx.send(Msg::GatheringComplete(generation));
            }
        })));
        let tx = self.tx.clone();
        pc.on_connection_state_change(Some(Box::new(move |state| {
            let _ = tx.send(Msg::ConnectionState(generation, state));
        })));
        let tx = self.tx.clone();
        pc.on_track(Some(Box::new(move |event| {
            let mid = event.transceiver.mid();
            let _ = tx.send(Msg::Track(generation, mid, event.track));
        })));
    }

    fn teardown(&mut self, mut call: Call) {
        log::info!("close");
        for task in call
            .timers
            .drain(..)
            .chain(call.pumps.drain(..))
            .chain(call.pcm_pumps.drain(..))
        {
            task.abort();
        }
        if let Some(camera) = call.camera.take() {
            camera.stop();
        }
        if let Some(screen) = call.screen.take() {
            screen.stop();
        }
        call.pc.close();
        if !self.synthetic {
            crate::audio::set_mic_tap(None);
            call.factory.release_platform_adm();
        }
        for mid in std::mem::take(&mut call.remote_videos) {
            let event = MediaEvent::Video {
                call_id: call.id.clone(),
                slot: VideoSlot::Remote(mid),
                present: false,
            };
            self.emit(event);
        }
        if call.reconnect_deadline.is_some() {
            self.emit(MediaEvent::Reconnecting {
                call_id: call.id.clone(),
                reconnecting: false,
            });
        }
        // The factory goes last, with the call: one call at a time, nothing else uses it.
    }

    fn emit_failure(&self, call_id: &str, reason: String) {
        log::warn!("failure: {reason}");
        self.emit(MediaEvent::Failed {
            call_id: call_id.into(),
            reason,
        });
    }

    fn fail_with(&mut self, key: &str, detail: impl std::fmt::Display) {
        self.fail(format!("{}{detail}", text(key)));
    }

    fn fail(&mut self, reason: String) {
        let Some(mut call) = self.call.take() else {
            return;
        };
        call.diagnostics.failure_reason = Some(reason.clone());
        let (id, diagnostics) = (call.id.clone(), call.diagnostics.clone());
        self.emit(MediaEvent::Diagnostics {
            call_id: id.clone(),
            diagnostics,
        });
        self.emit_failure(&id, reason);
        self.teardown(call);
    }

    /// NGC takes no trickled candidates: send once gathering completes, or after 3 s with what was gathered.
    fn wait_for_gathering(&mut self, pending: Pending) {
        let Some(call) = self.call.as_mut() else {
            return;
        };
        call.pending = pending;
        call.negotiation += 1;
        if call.pc.ice_gathering_state() == IceGatheringState::Complete {
            self.send_description();
            return;
        }
        let msg = Msg::GatheringTimeout(call.generation, call.negotiation);
        self.schedule(GATHERING_LIMIT, msg);
    }

    fn send_description(&mut self) {
        let Some(call) = self.call.as_mut() else {
            return;
        };
        if call.pending == Pending::None {
            return;
        }
        let Some(description) = call.pc.local_description() else {
            return;
        };
        let pending = std::mem::replace(&mut call.pending, Pending::None);
        let sdp = description.to_string();
        log::info!(
            "local SDP candidates={} relay={} gathering={:?}",
            sdp.matches("a=candidate:").count(),
            sdp.contains(" typ relay"),
            call.pc.ice_gathering_state()
        );
        let id = call.id.clone();
        // An incoming call (local answer) starts the deadline here; an outgoing one when the remote answer lands.
        if description.sdp_type() == SdpType::Answer {
            self.wait_for_connection();
        }
        self.emit(match pending {
            Pending::RenegotiationAnswer => MediaEvent::RenegotiationAnswer { call_id: id, sdp },
            _ => MediaEvent::LocalDescription { call_id: id, sdp },
        });
    }

    async fn remote_description(&mut self, call_id: &str, sdp: &str, provisional: bool) {
        log::info!(
            "remote SDP provisional={provisional} candidates={}",
            sdp.matches("a=candidate:").count()
        );
        let Some(call) = self.call_mut(call_id) else {
            return;
        };
        let (pc, generation) = (call.pc.clone(), call.generation);
        let kind = if provisional {
            SdpType::PrAnswer
        } else {
            SdpType::Answer
        };
        let result = set_remote(&pc, sdp, kind).await;
        if self.current(generation).is_none() {
            return;
        }
        match result {
            Err(e) => self.fail_with("media_answer_rejected", e),
            Ok(()) => {
                log::info!("remote SDP accepted");
                if !provisional {
                    self.wait_for_connection();
                }
            }
        }
    }

    fn wait_for_connection(&mut self) {
        let Some(call) = self.call.as_ref() else {
            return;
        };
        // While reconnecting, `reconnect_tick` owns the deadline.
        if call.pc.connection_state() == PeerConnectionState::Connected
            || call.reconnect_deadline.is_some()
        {
            return;
        }
        let msg = Msg::ConnectTimeout(call.generation, call.negotiation);
        self.schedule(CONNECT_LIMIT, msg);
    }

    /// A new local offer for the call as it now is: the core sends it as a renegotiation.
    async fn renegotiate(&mut self, reason: &str, ice_restart: bool) {
        let Some(call) = self.call.as_ref() else {
            return;
        };
        log::info!("renegotiate: {reason}");
        let (pc, generation) = (call.pc.clone(), call.generation);
        let Ok(offer) = pc.create_offer(offer_options(ice_restart)).await else {
            return;
        };
        if self.current(generation).is_none() || pc.set_local_description(offer).await.is_err() {
            return;
        }
        if self.current(generation).is_some() {
            self.wait_for_gathering(Pending::Local);
        }
    }

    /// A pending offer of ours (e.g. a refused camera) yields to the service's: one offer at a time.
    async fn remote_offer(&mut self, call_id: &str, sdp: &str) {
        let Some(call) = self.call_mut(call_id) else {
            return;
        };
        let (pc, generation) = (call.pc.clone(), call.generation);
        if !self.roll_back(&pc, generation).await {
            return;
        }
        let result = set_remote(&pc, sdp, SdpType::Offer).await;
        if self.current(generation).is_none() {
            return;
        }
        if let Err(e) = result {
            return self.fail_with("renegotiation_rejected", e);
        }
        let Ok(answer) = pc.create_answer(AnswerOptions::default()).await else {
            return;
        };
        if self.current(generation).is_none() || pc.set_local_description(answer).await.is_err() {
            return;
        }
        if self.current(generation).is_some() {
            self.wait_for_gathering(Pending::RenegotiationAnswer);
        }
    }

    /// Drops the refused offer, then offers again or restores the senders (web client's `rejectNegotiation`).
    async fn offer_rejected(&mut self, call_id: &str, retry: bool) {
        let Some(call) = self.call_mut(call_id) else {
            return;
        };
        log::info!("local offer rejected retry={retry}");
        let (pc, generation) = (call.pc.clone(), call.generation);
        if !self.roll_back(&pc, generation).await {
            return;
        }
        if retry {
            self.renegotiate("retry after rejection", false).await;
        } else {
            self.restore_negotiated_senders();
        }
    }

    /// Rolls back a pending local offer; false when the call ended meanwhile.
    async fn roll_back(&mut self, pc: &PeerConnection, generation: u64) -> bool {
        if pc.signaling_state() != SignalingState::HaveLocalOffer {
            return true;
        }
        log::info!("rolling back the pending local offer");
        if let Some(call) = self.current(generation) {
            call.pending = Pending::None;
        }
        let rollback = SessionDescription::parse("", SdpType::Rollback);
        let result = match rollback {
            Ok(rollback) => pc
                .set_local_description(rollback)
                .await
                .map_err(|e| e.message),
            Err(e) => Err(e.description),
        };
        if self.current(generation).is_none() {
            return false;
        }
        if let Err(e) = result {
            self.fail_with("renegotiation_rejected", e);
            return false;
        }
        true
    }

    /// Stops what the rejected offer meant to start sending, and tells the core.
    fn restore_negotiated_senders(&mut self) {
        let Some(call) = self.call.as_mut() else {
            return;
        };
        let id = call.id.clone();
        let mut events = Vec::new();
        if call.camera_track.is_some() && !sends(call.camera_line.as_ref()) {
            stop_camera(call);
            if let Some(line) = &call.camera_line {
                stop_sending(line);
            }
            events.push(MediaEvent::Video {
                call_id: id.clone(),
                slot: VideoSlot::Camera,
                present: false,
            });
            events.push(MediaEvent::CameraStopped {
                call_id: id.clone(),
                reason: Some(text("the_service_refused_the_camera")),
            });
        }
        if call.screen_track.is_some() && !sends(call.sharing_line.as_ref()) {
            stop_screen(call);
            if let Some(line) = &call.sharing_line {
                stop_sending(line);
            }
            events.push(MediaEvent::Video {
                call_id: id.clone(),
                slot: VideoSlot::Screen,
                present: false,
            });
            events.push(MediaEvent::ScreenShareStopped {
                call_id: id.clone(),
                reason: Some(text("the_service_refused_screen_sharing")),
            });
        }
        for event in events {
            self.emit(event);
        }
    }

    async fn connection_state(&mut self, generation: u64, state: PeerConnectionState) {
        log::info!("connection state={state:?}");
        let Some(call) = self.current(generation) else {
            return;
        };
        match state {
            PeerConnectionState::Connected => {
                let id = call.id.clone();
                let was_reconnecting = call.reconnect_deadline.take().is_some();
                call.reconnect_attempts = 0;
                call.diagnostics.connected = true;
                let first = !call.connected_once;
                call.connected_once = true;
                if was_reconnecting {
                    log::info!("reconnected");
                    self.emit(MediaEvent::Reconnecting {
                        call_id: id.clone(),
                        reconnecting: false,
                    });
                }
                if first {
                    let stats = self.every(Duration::from_secs(2), Msg::StatsTick, generation);
                    let speakers =
                        self.every(Duration::from_millis(250), Msg::SpeakersTick, generation);
                    if let Some(call) = self.current(generation) {
                        call.timers.extend([stats, speakers]);
                    }
                }
                self.emit(MediaEvent::Connected { call_id: id });
            }
            PeerConnectionState::Disconnected => call.diagnostics.disconnects += 1,
            PeerConnectionState::Failed if call.connected_once => self.reconnect(generation),
            PeerConnectionState::Failed => {
                call.diagnostics.failed = true;
                self.fail(text("the_audio_video_connection_failed_network"));
            }
            _ => {}
        }
    }

    /// After a drop, ICE restarts 1 s, 2 s… 5 s apart, for up to 90 s, like the web client.
    fn reconnect(&mut self, generation: u64) {
        let Some(call) = self.current(generation) else {
            return;
        };
        if call.reconnect_deadline.is_some() {
            return;
        }
        log::info!("reconnecting for up to {RECONNECT_LIMIT:?}");
        call.reconnect_deadline = Some(Instant::now() + RECONNECT_LIMIT);
        call.reconnect_attempts = 0;
        call.reconnect_next = Instant::now();
        let id = call.id.clone();
        self.emit(MediaEvent::Reconnecting {
            call_id: id,
            reconnecting: true,
        });
        let _ = self.tx.send(Msg::ReconnectTick(generation));
    }

    async fn reconnect_tick(&mut self, generation: u64) {
        let Some(call) = self.current(generation) else {
            return;
        };
        let Some(deadline) = call.reconnect_deadline else {
            return;
        };
        let now = Instant::now();
        if now >= deadline {
            call.diagnostics.failed = true;
            return self.fail(text("the_audio_video_connection_failed_network"));
        }
        // No offer in flight and ICE no longer checking: the previous attempt, if any, is over.
        let settled = call.pc.signaling_state() == SignalingState::Stable
            && !matches!(
                call.pc.connection_state(),
                PeerConnectionState::Connecting | PeerConnectionState::Connected
            );
        if settled && now >= call.reconnect_next {
            call.reconnect_attempts += 1;
            call.reconnect_next =
                now + Duration::from_secs(u64::from(call.reconnect_attempts.min(5)));
            log::info!("reconnect attempt {}", call.reconnect_attempts);
            self.renegotiate("ICE restart", true).await;
        }
        let Some(call) = self.current(generation) else {
            return;
        };
        if call.reconnect_deadline.is_some() {
            self.schedule(Duration::from_millis(500), Msg::ReconnectTick(generation));
        }
    }

    /// Every 250 ms: the CSRCs the mixer currently mixes in, and our own input level.
    async fn poll_speakers(&mut self, generation: u64) {
        let Some(call) = self.current(generation) else {
            return;
        };
        let now = Instant::now();
        let receivers: Vec<_> = call
            .pc
            .receivers()
            .into_iter()
            .filter(|r| matches!(r.track(), Some(MediaStreamTrack::Audio(_))))
            .collect();
        for source in receivers
            .iter()
            .flat_map(|r| r.sources())
            .filter(|s| s.csrc)
        {
            let previous = call
                .source_stamps
                .insert(source.source_id, source.timestamp_us);
            let audible = source.audio_level.is_none_or(|level| level < 127);
            if previous.is_some_and(|p| p != source.timestamp_us) && audible {
                call.last_heard.insert(source.source_id, now);
            }
        }
        call.last_heard
            .retain(|_, heard| now.duration_since(*heard) < SPEAKING_HOLD);
        let speaking: BTreeSet<u32> = call.last_heard.keys().copied().collect();
        let id = call.id.clone();
        if speaking != call.speaking {
            call.speaking = speaking.clone();
            self.emit(MediaEvent::Speaking {
                call_id: id.clone(),
                sources: speaking,
            });
        }
        let Some(call) = self.current(generation) else {
            return;
        };
        let (pc, muted) = (call.pc.clone(), call.muted);
        let Ok(report) = pc.get_stats().await else {
            return;
        };
        let Some(call) = self.current(generation) else {
            return;
        };
        let speaking = !muted && sample(&report).mic_level > 0.05;
        if speaking != call.local_speaking {
            call.local_speaking = speaking;
            self.emit(MediaEvent::LocalSpeaking {
                call_id: id,
                speaking,
            });
        }
    }

    fn start_camera(&self, call: &mut Call) -> Option<RtcVideoTrack> {
        let tx = self.tx.clone();
        let generation = call.generation;
        let start = if self.synthetic {
            Camera::synthetic
        } else {
            Camera::start
        };
        let Some((camera, track)) = start(
            &call.factory,
            self.devices.camera.as_ref(),
            self.sinks.clone(),
            Box::new(move |width, height| {
                let _ = tx.send(Msg::FrameSize(generation, VideoSlot::Camera, width, height));
            }),
            {
                let tx = self.tx.clone();
                Box::new(move || {
                    let _ = tx.send(Msg::CaptureFailed(generation, VideoSlot::Camera));
                })
            },
        ) else {
            log::warn!("camera unavailable");
            call.diagnostics.camera_failed = true;
            return None;
        };
        log::info!("camera on");
        call.camera = Some(camera);
        call.camera_track = Some(track.clone());
        self.emit(MediaEvent::Video {
            call_id: call.id.clone(),
            slot: VideoSlot::Camera,
            present: true,
        });
        Some(track)
    }

    /// The camera line changes direction and renegotiates (web client's `startRenegotiationAsync`).
    async fn set_camera(&mut self, call_id: &str, on: bool) {
        let Some(call) = self.call_mut(call_id) else {
            return;
        };
        // Before the first answer, or with a negotiation in flight: only the track changes.
        if call.pc.current_remote_description().is_none()
            || call.pc.signaling_state() != SignalingState::Stable
        {
            if let Some(track) = &call.camera_track {
                track.set_enabled(on);
            }
            return;
        }
        let Some(line) = video_line(call, false) else {
            return;
        };
        if on {
            if call.camera_track.is_none() {
                let mut taken = self.call.take().expect("call");
                let started = self.start_camera(&mut taken);
                self.call = Some(taken);
                if started.is_none() {
                    return;
                }
            }
            let Some(call) = self.call.as_mut() else {
                return;
            };
            let Some(track) = call.camera_track.clone() else {
                return;
            };
            track.set_enabled(true);
            send_on(&line, track, RtpTransceiverDirection::SendRecv);
            call.camera_line = Some(line);
        } else {
            let id = call.id.clone();
            stop_camera(call);
            stop_sending(&line);
            call.camera_line = Some(line);
            self.emit(MediaEvent::Video {
                call_id: id,
                slot: VideoSlot::Camera,
                present: false,
            });
        }
        self.renegotiate(if on { "camera on" } else { "camera off" }, false)
            .await;
    }

    async fn set_screen_share(&mut self, call_id: &str, on: bool, source: Option<ScreenSource>) {
        let tx = self.tx.clone();
        let sinks = self.sinks.clone();
        let Some(call) = self
            .call_mut(call_id)
            .filter(|c| c.pc.signaling_state() == SignalingState::Stable)
        else {
            if on {
                let reason = Some(text("the_media_connection_isn_t_ready"));
                self.emit(MediaEvent::ScreenShareStopped {
                    call_id: call_id.into(),
                    reason,
                });
            }
            return;
        };
        let id = call.id.clone();
        if on {
            stop_screen(call);
            let generation = call.generation;
            let size_tx = tx.clone();
            let started = ScreenCapture::start(
                &call.factory,
                source,
                sinks,
                Box::new(move |width, height| {
                    let _ =
                        size_tx.send(Msg::FrameSize(generation, VideoSlot::Screen, width, height));
                }),
                Box::new(move || {
                    let _ = tx.send(Msg::CaptureFailed(generation, VideoSlot::Screen));
                }),
            );
            let Some((capture, track)) = started else {
                let reason = Some(text("allow_oxide_in_system_settings_privacy"));
                self.emit(MediaEvent::ScreenShareStopped {
                    call_id: id,
                    reason,
                });
                return;
            };
            call.screen = Some(capture);
            call.screen_track = Some(track.clone());
            // Send-only like the web client: offered both ways, the mixer never takes our screen.
            match video_line(call, true) {
                Some(line) => {
                    send_on(&line, track, RtpTransceiverDirection::SendOnly);
                    call.sharing_line = Some(line);
                }
                None => {
                    let init = send_init(RtpTransceiverDirection::SendOnly);
                    call.sharing_line = call
                        .pc
                        .add_transceiver(MediaStreamTrack::Video(track), init)
                        .ok();
                }
            }
            self.emit(MediaEvent::Video {
                call_id: id,
                slot: VideoSlot::Screen,
                present: true,
            });
        } else {
            stop_screen(call);
            let Some(line) = video_line(call, true) else {
                return;
            };
            stop_sending(&line);
            self.emit(MediaEvent::Video {
                call_id: id,
                slot: VideoSlot::Screen,
                present: false,
            });
        }
        self.renegotiate(
            if on {
                "screen share on"
            } else {
                "screen share off"
            },
            false,
        )
        .await;
    }
}

fn rtc_ice_server(server: &IceServer) -> RtcIceServer {
    RtcIceServer {
        urls: server.urls.clone(),
        username: server.username.clone().unwrap_or_default(),
        password: server.credential.clone().unwrap_or_default(),
    }
}

/// `None`: libwebrtc's legacy `offerToReceive*` would turn receive-only lines inactive.
fn offer_options(ice_restart: bool) -> OfferOptions {
    OfferOptions {
        ice_restart,
        offer_to_receive_audio: None,
        offer_to_receive_video: None,
    }
}

/// Our audio or camera: its own send-receive line when offering, else the line the remote offer made.
fn add_track(
    pc: &PeerConnection,
    track: MediaStreamTrack,
    offering: bool,
) -> Option<RtpTransceiver> {
    if offering {
        let init = send_init(RtpTransceiverDirection::SendRecv);
        return pc.add_transceiver(track, init).ok();
    }
    let id = track.id();
    pc.add_track(track, &["local"]).ok()?;
    pc.transceivers()
        .into_iter()
        .find(|t| t.sender().track().is_some_and(|s| s.id() == id))
}

fn add_receive_video(pc: &PeerConnection) -> Option<RtpTransceiver> {
    let init = RtpTransceiverInit {
        direction: RtpTransceiverDirection::RecvOnly,
        stream_ids: vec![],
        send_encodings: vec![],
    };
    pc.add_transceiver_for_media(MediaType::Video, init).ok()
}

fn send_init(direction: RtpTransceiverDirection) -> RtpTransceiverInit {
    RtpTransceiverInit {
        direction,
        stream_ids: vec!["local".into()],
        send_encodings: vec![RtpEncodingParameters::default()],
    }
}

fn send_on(line: &RtpTransceiver, track: RtcVideoTrack, direction: RtpTransceiverDirection) {
    let _ = line
        .sender()
        .set_track(Some(MediaStreamTrack::Video(track)));
    let _ = line.set_direction(direction);
}

fn stop_sending(line: &RtpTransceiver) {
    let _ = line.sender().set_track(None);
    let _ = line.set_direction(RtpTransceiverDirection::RecvOnly);
}

/// Parses and applies the remote side's SDP, read with the H264 profile we send.
async fn set_remote(pc: &PeerConnection, sdp: &str, kind: SdpType) -> Result<(), String> {
    let description =
        SessionDescription::parse(&send_profile(sdp), kind).map_err(|e| e.description)?;
    pc.set_remote_description(description)
        .await
        .map_err(|e| e.message)
}

/// macOS: Constrained High instead of Teams' Baseline, as VideoToolbox rate-controls for calls only in High.
fn send_profile(sdp: &str) -> String {
    if !cfg!(target_os = "macos") {
        return sdp.to_string();
    }
    sdp.replace("profile-level-id=42C02A", "profile-level-id=640C2A")
        .replace("profile-level-id=42c02a", "profile-level-id=640c2a")
}

/// The last negotiation lets this line send.
fn sends(line: Option<&RtpTransceiver>) -> bool {
    line.and_then(|l| l.current_direction()).is_some_and(|d| {
        matches!(
            d,
            RtpTransceiverDirection::SendRecv | RtpTransceiverDirection::SendOnly
        )
    })
}

/// The remembered video line, else by position (an incoming call's lines come from the caller's offer).
fn video_line(call: &Call, sharing: bool) -> Option<RtpTransceiver> {
    let remembered = if sharing {
        &call.sharing_line
    } else {
        &call.camera_line
    };
    if let Some(line) = remembered
        .as_ref()
        .filter(|l| l.direction() != RtpTransceiverDirection::Stopped)
    {
        return Some(line.clone());
    }
    let lines: Vec<_> = call
        .pc
        .transceivers()
        .into_iter()
        .filter(|t| {
            t.receiver()
                .track()
                .is_some_and(|t| matches!(t, MediaStreamTrack::Video(_)))
        })
        .filter(|t| t.direction() != RtpTransceiverDirection::Stopped)
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

fn stop_camera(call: &mut Call) {
    if let Some(camera) = call.camera.take() {
        camera.stop();
    }
    call.camera_track = None;
    call.frame_sizes.remove(&VideoSlot::Camera);
}

fn stop_screen(call: &mut Call) {
    if let Some(screen) = call.screen.take() {
        screen.stop();
    }
    call.screen_track = None;
    call.frame_sizes.remove(&VideoSlot::Screen);
}

/// Web client's `updateEncodingsWithCaps` / `generateKeyFrameIfNeeded`, screens capped at 8160 fs, 15 fps.
fn apply_limits(call: &Call, limits: &SendLimits, keyframe: bool) {
    let mid = limits.mid.as_str();
    let Some(line) = call
        .pc
        .transceivers()
        .into_iter()
        .find(|t| t.mid().as_deref() == Some(mid))
    else {
        return;
    };
    let sender = line.sender();
    if sender.track().is_none() {
        return;
    }
    let slot = if call.sharing_line.as_ref().and_then(|l| l.mid()).as_deref() == Some(mid) {
        VideoSlot::Screen
    } else {
        VideoSlot::Camera
    };
    let Some(&(width, height)) = call.frame_sizes.get(&slot) else {
        return;
    };
    if width == 0 {
        return;
    }
    let (max_fs, fps) = if limits.sharing {
        (
            limits.max_fs.min(SCREEN_MAX_FS),
            limits.max_fps.min(SCREEN_MAX_FPS),
        )
    } else {
        (limits.max_fs, limits.max_fps)
    };
    let scale = scale_down(width, height, max_fs);
    let mut parameters = sender.parameters();
    for encoding in &mut parameters.encodings {
        encoding.scale_resolution_down_by = Some(scale);
        encoding.max_framerate = Some(fps);
        if limits.max_bitrate > 0 {
            encoding.max_bitrate = Some(u64::from(limits.max_bitrate));
        }
    }
    log::info!(
        "send limits mid={mid} {width}x{height} fs={max_fs} scale={scale:.2} fps={fps} br={} keyframe={keyframe}",
        limits.max_bitrate
    );
    if let Err(e) = sender.set_parameters(parameters) {
        log::warn!("send limits mid={mid}: {e:?}");
    }
    if keyframe {
        sender.generate_key_frame();
    }
}

/// How much to shrink a `width`×`height` frame so it fits in `max_fs` macroblocks.
fn scale_down(width: u32, height: u32, max_fs: u32) -> f64 {
    let pixels = f64::from(width) * f64::from(height);
    (pixels / (f64::from(max_fs.max(1)) * 256.0))
        .sqrt()
        .max(1.0)
}

/// After a device change the audio device must restart to use it.
fn restart_audio(factory: &PeerConnectionFactory) {
    if factory.recording_is_initialized() {
        factory.stop_recording();
        factory.init_recording();
        factory.start_recording();
    }
    if factory.playout_is_initialized() {
        factory.stop_playout();
        factory.init_playout();
        factory.start_playout();
    }
}

#[cfg(test)]
mod tests {
    use super::scale_down;

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_sends_constrained_high_for_videotoolbox_low_latency() {
        let answer = "a=fmtp:107 profile-level-id=42C02A;packetization-mode=1\r\n";
        assert_eq!(
            super::send_profile(answer),
            "a=fmtp:107 profile-level-id=640C2A;packetization-mode=1\r\n"
        );
    }

    #[test]
    fn scale_fits_the_mixer_frame_size() {
        assert_eq!(scale_down(1280, 720, 3600), 1.0);
        let scale = scale_down(1280, 720, 920);
        assert!((scale - 1.978).abs() < 0.01, "{scale}");
        assert_eq!(scale_down(640, 360, 8160), 1.0);
    }
}

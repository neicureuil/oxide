//! The one active call: TURN credentials, native local media, NGC signalling and push-routed callbacks.
use super::*;
use serde_json::Value;
use teams_calling::client::ConversationClient;
use teams_calling::payload::{self, Callbacks, Local, Media, Target};
use teams_calling::session::{
    CallKind, CallSession, Command, EndReason, Input, Op, Phase, RecorderState, Recording, Timer,
};
use teams_calling::trap::{self, TrapClient};
use teams_calling::{MeetingLink, sdp};
use teams_i18n::{t, tf};

/// The offer has a live, sending video line.
fn offers_video(sdp: &str) -> bool {
    sdp.split("m=").skip(1).any(|section| {
        section.starts_with("video ")
            && !section.starts_with("video 0 ")
            && !section.contains("a=inactive")
            && !section.contains("a=recvonly")
    })
}

/// `teamsEchoBotMri` when the web client's configuration was never read.
const ECHO_BOT_MRI: &str = "28:cf28171e-fcfd-47e4-a1d6-79460b0b3ca0";

/// The Echo Bot of the web client's configuration.
pub(crate) fn echo_bot_mri() -> String {
    teams_auth::web_config::current()
        .and_then(|config| config.echo_bot_mri)
        .unwrap_or_else(|| ECHO_BOT_MRI.to_owned())
}

/// Cameras shown at once in a group call, one video line each (web `numVideoChannelsGvc`, `nonMultiStreamChannels` 9).
const CAMERA_LINES: u32 = 8;

/// `getRecordingFileName`: `<title>-<local yyyyMMdd_HHmmss>-Meeting Recording`.
fn recording_file_name(title: &str) -> String {
    let title: String = title
        .chars()
        .map(|c| if "\\/:*?\"<>|#%".contains(c) { '_' } else { c })
        .collect();
    format!(
        "{}-{}-Meeting Recording",
        title.trim(),
        chrono::Local::now().format("%Y%m%d_%H%M%S")
    )
}

/// Members rung by a group call from a chat; bigger groups should use a meeting.
const MAX_GROUP_CALL: usize = 20;

#[derive(Debug, Clone, PartialEq, Eq, Hash, uniffi::Record)]
pub struct IceServerView {
    pub urls: Vec<String>,
    pub username: Option<String>,
    pub credential: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum CallPhaseView {
    /// Someone is calling: accept or decline.
    Incoming,
    /// Fetching relays and preparing local media.
    Preparing,
    Connecting,
    Ringing,
    Lobby,
    Connected,
    Ending,
    Ended,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, uniffi::Record)]
pub struct ActiveCallView {
    pub id: String,
    pub title: String,
    pub phase: CallPhaseView,
    /// French label of the phase, or the reason the call ended.
    pub status: String,
    pub muted: bool,
    pub video: bool,
    pub media_connected: bool,
    /// Local camera currently sending.
    pub camera: bool,
    /// We share our screen.
    pub sharing_screen: bool,
    /// The other participants are heard on this device.
    pub speaker: bool,
    pub participants: Vec<CallParticipantView>,
    pub videos: Vec<CallVideoView>,
    /// When the call connected (ms since the epoch), for the duration shown.
    pub connected_at_ms: Option<i64>,
    /// Recording or transcription consent notice, when the meeting enforces it.
    pub notice: Option<String>,
    /// Test call with the Echo Bot: the window shows a report at the end.
    pub is_test: bool,
    pub is_meeting: bool,
    /// Incoming call: the caller's offer has arrived, so it can be answered.
    pub can_accept: bool,
    /// Incoming call offered with video.
    pub offers_video: bool,
    /// Cloud recording of the call, whoever started it.
    pub recording: RecordingView,
    /// We may ask the service to record (not an attendee, meeting allows it).
    pub can_record: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum RecordingView {
    None,
    /// The recorder is being added or told to start.
    Starting,
    /// Recording; `by` names who started it, when known.
    Active { by: Option<String> },
    Stopping,
    /// Our request to record failed.
    Failed { reason: String },
}

/// A participant's camera or shared screen, received on one of our video lines.
#[derive(Debug, Clone, PartialEq, Eq, Hash, uniffi::Record)]
pub struct CallVideoView {
    /// m-line of the remote track (`RTCRtpTransceiver.mid`).
    pub mid: String,
    pub participant_id: String,
    pub sharing: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, uniffi::Record)]
pub struct CallParticipantView {
    pub id: String,
    pub name: String,
    /// `organizer`, `presenter`, `attendee` when known.
    pub role: String,
    pub in_lobby: bool,
    /// CSRCs of their voice in the mixed audio, to show who is speaking.
    pub audio_sources: Vec<u32>,
    pub muted: bool,
}

/// Before joining a meeting: choose camera and microphone, then join.
#[derive(Debug, Clone, PartialEq, Eq, Hash, uniffi::Record)]
pub struct PrejoinView {
    pub title: String,
    /// Our other devices already in this meeting: join on this one too, or move here.
    pub other_devices: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, uniffi::Record)]
pub struct CallPart {
    pub active: Option<ActiveCallView>,
    pub prejoin: Option<PrejoinView>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, uniffi::Enum)]
pub enum CallAction {
    /// 1:1 call to an MRI (`8:orgid:…`).
    Start {
        mri: String,
        video: bool,
    },
    /// Calls the members of a chat (`None`: the selected one): 1:1, or a group call tied to the chat.
    CallChat {
        chat_id: Option<String>,
        video: bool,
    },
    /// Meeting from its join link.
    JoinMeeting {
        url: String,
        video: bool,
    },
    /// Opens the pre-join screen for a meeting link (calendar).
    PrepareMeeting {
        url: String,
        title: String,
    },
    /// Opens the pre-join screen for the selected meeting chat.
    PrepareChatMeeting,
    /// Joins the meeting shown on the pre-join screen. `transfer`: our other devices leave.
    ConfirmJoin {
        video: bool,
        muted: bool,
        speaker: bool,
        transfer: bool,
    },
    CancelJoin,
    SetCamera {
        on: bool,
    },
    /// Starts or stops sharing our screen: the media engine renegotiates.
    SetScreenShare {
        on: bool,
    },
    /// The engine stopped sharing on its own (permission refused, display gone).
    ScreenShareStopped {
        call_id: String,
        reason: Option<String>,
    },
    /// The engine stopped the camera on its own (the service rejected it).
    CameraStopped {
        call_id: String,
        reason: Option<String>,
    },
    SetSpeaker {
        on: bool,
    },
    /// Answers the incoming call.
    Accept {
        video: bool,
    },
    Decline,
    /// Test call with the Echo Bot (Settings); `bot_mri` overrides the web config's bot.
    TestCall {
        bot_mri: Option<String>,
    },
    /// Demo mode only: someone calls.
    SimulateIncoming {
        video: bool,
    },
    Hangup,
    SetMuted {
        muted: bool,
    },
    /// The native shell closed the call window after the end.
    Dismiss,
    /// Local SDP (offer or answer), ICE gathering complete.
    LocalDescription {
        call_id: String,
        sdp: String,
    },
    MediaConnected {
        call_id: String,
    },
    MediaFailed {
        call_id: String,
        reason: String,
    },
    /// Our answer to `Effect::MediaRemoteOffer`.
    RenegotiationAnswer {
        call_id: String,
        sdp: String,
    },
    /// Cloud recording (OneDrive `Recordings`), as the web client's Record button.
    StartRecording,
    StopRecording,
    /// The Mac woke up or changed network: reconnect the push channel and restart ICE.
    NetworkChanged,
}

#[derive(Debug, Clone)]
pub(crate) enum CallMsg {
    Action(CallAction),
    Relays(u64, String, Vec<IceServerView>),
    EchoChat(u64, String, Result<String, String>),
    Http(u64, String, Op, Result<Value, u16>),
    Timer(u64, String, Timer),
    /// AAD token for the recorder bot, asked by `StartRecording`.
    RecorderToken(u64, String, Result<String, String>),
}

/// WebRTC mid of a line the service names: a clashing line of its offer was renamed.
fn webrtc_mid(remote_offer: Option<&str>, local: Option<&str>, mid: String) -> String {
    remote_offer
        .map(|offer| sdp::mid_renames(offer, local))
        .unwrap_or_default()
        .into_iter()
        .find(|(ngc, _)| *ngc == mid)
        .map_or(mid, |(_, webrtc)| webrtc)
}

impl ActiveCall {
    fn new(id: String, title: String, kind: CallKind, video: bool, muted: bool) -> Self {
        Self {
            id,
            title,
            kind,
            video,
            session: None,
            local_sdp: None,
            ended: None,
            media_connected: false,
            muted,
            camera: video,
            screen: false,
            speaker: true,
            replace_other_devices: false,
            connected_at: None,
            remote_offer: None,
            remote_ngc_offer: None,
            recorder_token_pending: false,
            recording_error: None,
        }
    }

    /// Not ended, and its session is in `phase`.
    fn in_phase(&self, phase: Phase) -> bool {
        self.ended.is_none() && self.session.as_ref().is_some_and(|s| s.phase == phase)
    }

    /// A meeting or group call shows several cameras: it offers their video lines.
    fn group(&self) -> bool {
        match &self.kind {
            CallKind::Meeting(_) => true,
            CallKind::Outgoing { to, .. } => to.len() > 1,
            _ => false,
        }
    }

    /// `callModalities` include Video when we receive cameras, or the mixer rejects their lines.
    fn media_video(&self) -> bool {
        self.video || self.group()
    }

    fn webrtc_mid(&self, mid: String) -> String {
        webrtc_mid(
            self.remote_ngc_offer.as_deref(),
            self.local_sdp.as_deref(),
            mid,
        )
    }

    /// The mixer hosts the call (`isConference` SDP rules): group, chat-tied, or any call it accepted itself.
    fn conference(&self) -> bool {
        self.group()
            || matches!(
                &self.kind,
                CallKind::Outgoing {
                    chat_thread: Some(_),
                    ..
                } | CallKind::EchoTest { .. }
            )
            || self.session.as_ref().is_some_and(CallSession::hosted)
            || self
                .remote_ngc_offer
                .as_deref()
                .is_some_and(|offer| offer.contains("a=x-source-streamid:"))
    }
}

pub(crate) struct ActiveCall {
    id: String,
    title: String,
    kind: CallKind,
    video: bool,
    /// Created once local media is ready.
    session: Option<CallSession>,
    /// Our WebRTC offer, to restore what the service's answer leaves out.
    local_sdp: Option<String>,
    ended: Option<EndReason>,
    media_connected: bool,
    muted: bool,
    camera: bool,
    screen: bool,
    speaker: bool,
    /// Applied to the session once created.
    replace_other_devices: bool,
    connected_at: Option<std::time::SystemTime>,
    /// Incoming call: the caller's offer, as WebRTC SDP, until we answer.
    remote_offer: Option<String>,
    /// The service's latest NGC offer: our answer mirrors its lines.
    remote_ngc_offer: Option<String>,
    /// `StartRecording` waits for the recorder's token.
    recorder_token_pending: bool,
    /// Why our request to record failed before reaching the session.
    recording_error: Option<String>,
}

#[derive(Default)]
pub(crate) struct CallState {
    pub(crate) active: Option<ActiveCall>,
    prejoin: Option<Prejoin>,
    error: Option<String>,
    echo_chat: Option<(String, String)>,
    /// Built once per session, like the calendar client.
    client: std::sync::OnceLock<ConversationClient<AppAuthenticator>>,
}

/// Pre-join screen of a meeting, following its roster to spot our other devices in it.
struct Prejoin {
    kind: CallKind,
    title: String,
    watch: Option<CallSession>,
}

/// A session's request or timer, answered as `CallMsg::Http` or `CallMsg::Timer`.
fn session_task(
    client: Option<&Client>,
    g: u64,
    id: String,
    command: Command,
) -> Option<Task<Msg>> {
    match command {
        Command::Http {
            op,
            method,
            url,
            body,
        } => {
            let client = ConversationClient::new(client?.authenticator());
            let chain = id.clone();
            Some(Task::perform(
                async move { client.send_op(&chain, op, method, &url, &body).await },
                move |result| Msg::Call(CallMsg::Http(g, id, op, result)),
            ))
        }
        Command::Timer(timer, delay) => Some(Task::perform(
            async move { tokio::time::sleep(delay).await },
            move |()| Msg::Call(CallMsg::Timer(g, id, timer)),
        )),
        _ => None,
    }
}

fn phase_view(call: &ActiveCall) -> (CallPhaseView, String) {
    if let Some(reason) = &call.ended {
        return (CallPhaseView::Ended, reason.message());
    }
    let Some(session) = &call.session else {
        return (
            CallPhaseView::Preparing,
            t("core.call.preparing_audio").into(),
        );
    };
    match &session.phase {
        Phase::Notified => (CallPhaseView::Incoming, t("core.call.incoming_call").into()),
        Phase::Connecting => (CallPhaseView::Connecting, t("core.call.connecting").into()),
        Phase::Ringing => (CallPhaseView::Ringing, t("core.call.ringing").into()),
        Phase::Lobby => (
            CallPhaseView::Lobby,
            t("core.call.in_the_lobby_an_organizer_must").into(),
        ),
        Phase::Connected if call.media_connected => {
            (CallPhaseView::Connected, t("core.call.in_call").into())
        }
        Phase::Connected => (
            CallPhaseView::Connected,
            t("core.call.connecting_audio").into(),
        ),
        Phase::Ending => (CallPhaseView::Ending, t("core.call.ending_call").into()),
        Phase::Ended(reason) => (CallPhaseView::Ended, reason.message()),
    }
}

impl App {
    pub(crate) fn call_part(&self) -> CallPart {
        CallPart {
            active: self.call.active.as_ref().map(|call| {
                let (phase, status) = phase_view(call);
                ActiveCallView {
                    id: call.id.clone(),
                    title: call.title.clone(),
                    phase,
                    status,
                    muted: call.muted,
                    video: call.video,
                    media_connected: call.media_connected,
                    camera: call.camera,
                    sharing_screen: call.screen,
                    speaker: call.speaker,
                    participants: call
                        .session
                        .as_ref()
                        .map(|s| {
                            s.roster
                                .values()
                                .filter(|p| !p.id.eq_ignore_ascii_case(&self.me))
                                // The recorder bot is not a person.
                                .filter(|p| !payload::recording::is_bot(&p.id))
                                .map(|p| CallParticipantView {
                                    id: p.id.clone(),
                                    name: p
                                        .display_name
                                        .clone()
                                        .or_else(|| self.known_name(&p.id))
                                        .unwrap_or_else(|| "Participant".into()),
                                    role: p.role.clone().unwrap_or_default().to_ascii_lowercase(),
                                    in_lobby: p.in_lobby,
                                    audio_sources: p.audio_sources.clone(),
                                    muted: p.muted,
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                    videos: call
                        .session
                        .as_ref()
                        .map(|s| {
                            s.video_views()
                                .into_iter()
                                .map(|(mid, participant_id, sharing)| CallVideoView {
                                    mid: call.webrtc_mid(mid),
                                    participant_id,
                                    sharing,
                                })
                                .collect()
                        })
                        .unwrap_or_default(),
                    connected_at_ms: call.connected_at.and_then(|t| {
                        t.duration_since(std::time::UNIX_EPOCH)
                            .ok()
                            .map(|d| d.as_millis() as i64)
                    }),
                    notice: call
                        .session
                        .as_ref()
                        .filter(|s| s.consent_required())
                        .map(|_| t("core.call.this_meeting_may_be_recorded_or").into()),
                    is_meeting: matches!(call.kind, CallKind::Meeting(_)),
                    is_test: matches!(call.kind, CallKind::EchoTest { .. }),
                    can_accept: call.remote_offer.is_some()
                        && call.ended.is_none()
                        && call
                            .session
                            .as_ref()
                            .is_some_and(|s| s.phase == Phase::Notified),
                    offers_video: call.remote_offer.as_deref().is_some_and(offers_video),
                    recording: self.recording_view(call),
                    can_record: self.can_record(call),
                }
            }),
            prejoin: self.call.prejoin.as_ref().map(|p| PrejoinView {
                title: p.title.clone(),
                other_devices: p
                    .watch
                    .as_ref()
                    .map_or(0, |w| w.other_devices().len() as u32),
            }),
            error: self.call.error.clone(),
        }
    }

    fn recording_view(&self, call: &ActiveCall) -> RecordingView {
        let Some(session) = call.session.as_ref().filter(|_| call.ended.is_none()) else {
            return RecordingView::None;
        };
        let recorder = session.recorder();
        match &session.recording {
            Recording::Inviting(_) | Recording::Starting => return RecordingView::Starting,
            Recording::Stopping => return RecordingView::Stopping,
            Recording::Failed if recorder.is_none_or(|r| r.state != RecorderState::Active) => {
                return RecordingView::Failed {
                    reason: t("core.call.recording_could_not_start").into(),
                };
            }
            _ => {}
        }
        if call.recorder_token_pending {
            return RecordingView::Starting;
        }
        if let Some(reason) = &call.recording_error {
            return RecordingView::Failed {
                reason: reason.clone(),
            };
        }
        match recorder.map(|r| (r.state, r.user_id.clone())) {
            Some((RecorderState::Active | RecorderState::Paused, by)) => RecordingView::Active {
                by: by.map(|id| {
                    if id.eq_ignore_ascii_case(&self.me) {
                        t("core.call.you").into()
                    } else {
                        session
                            .roster
                            .get(&id)
                            .and_then(|p| p.display_name.clone())
                            .or_else(|| self.known_name(&id))
                            .unwrap_or(id)
                    }
                }),
            },
            Some((RecorderState::WaitingForMeetingLive, _)) => RecordingView::Starting,
            _ => RecordingView::None,
        }
    }

    /// `getRecordingDisabledReason`, as far as the client knows: attendees and test calls cannot,
    /// nor meetings whose `meetingCapability` forbids it; the service decides the rest.
    fn can_record(&self, call: &ActiveCall) -> bool {
        let Some(session) = call.session.as_ref() else {
            return false;
        };
        let attendee = session
            .roster
            .get(self.me.as_str())
            .and_then(|me| me.role.as_deref())
            .is_some_and(|role| role.eq_ignore_ascii_case("attendee"));
        let forbidden = session
            .meeting_details
            .as_ref()
            .and_then(|d| d.pointer("/meetingCapability/allowCloudRecording"))
            .and_then(Value::as_bool)
            == Some(false);
        call.ended.is_none()
            && session.phase == Phase::Connected
            && !matches!(call.kind, CallKind::EchoTest { .. })
            && !attendee
            && !forbidden
    }

    /// The organizer's name, for the recording's metadata.
    fn meeting_organizer(&self, call: &ActiveCall) -> String {
        let session = call.session.as_ref();
        session
            .and_then(|s| {
                s.roster.values().find(|p| {
                    p.role
                        .as_deref()
                        .is_some_and(|r| r.eq_ignore_ascii_case("organizer"))
                })
            })
            .and_then(|p| p.display_name.clone().or_else(|| self.known_name(&p.id)))
            .or_else(|| match &call.kind {
                CallKind::Meeting(meeting) => meeting
                    .organizer_id
                    .as_ref()
                    .and_then(|id| self.known_name(&format!("8:orgid:{id}"))),
                _ => None,
            })
            .unwrap_or_else(|| self.display_name.clone())
    }

    fn start_recording(&mut self) -> Task<Msg> {
        let g = self.generation;
        let Some(client) = self.client.clone() else {
            return Task::none();
        };
        let Some(call) = self.call.active.as_mut().filter(|c| c.in_phase(Phase::Connected))
        else {
            return Task::none();
        };
        if call.recorder_token_pending {
            return Task::none();
        }
        call.recorder_token_pending = true;
        call.recording_error = None;
        let id = call.id.clone();
        let auth = client.authenticator();
        Task::perform(
            async move {
                let mut headers = http::HeaderMap::new();
                teams_auth::Authenticator::authorize(
                    &*auth,
                    teams_auth::Service::CallRecorder,
                    &mut headers,
                )
                    .await
                    .map_err(|e| e.to_string())?;
                Ok(headers
                    .get("authorization")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.strip_prefix("Bearer "))
                    .unwrap_or_default()
                    .to_owned())
            },
            move |result| Msg::Call(CallMsg::RecorderToken(g, id, result)),
        )
    }

    fn recorder_token(&mut self, g: u64, id: &str, result: Result<String, String>) -> Task<Msg> {
        if !self.is_active(g, id) {
            return Task::none();
        }
        let organizer = match self.call.active.as_ref() {
            Some(call) => self.meeting_organizer(call),
            None => return Task::none(),
        };
        let Some(call) = self.call.active.as_mut() else {
            return Task::none();
        };
        call.recorder_token_pending = false;
        let token = match result {
            Ok(token) => token,
            Err(error) => {
                call.recording_error = Some(tf!("core.call.recording_failed", error = error));
                return Task::none();
            }
        };
        let request = payload::recording::Request {
            token,
            title: call.title.clone(),
            organizer,
            ical_uid: None,
            file_name: recording_file_name(&call.title),
            correlation_id: uuid::Uuid::new_v4().to_string(),
        };
        self.step(Input::StartRecording(request))
    }

    fn conversation_client(&self) -> Option<ConversationClient<AppAuthenticator>> {
        let chat = self.client.as_ref()?;
        Some(
            self.call
                .client
                .get_or_init(|| ConversationClient::new(chat.authenticator()))
                .clone(),
        )
    }

    fn begin_call(&mut self, kind: CallKind, title: String, video: bool, muted: bool) -> Task<Msg> {
        let meeting_chat = match &kind {
            CallKind::Meeting(meeting) => Some(meeting.thread_id.clone()),
            _ => None,
        };
        let task = self.start_call(kind, title, video, muted);
        // Joining unmutes a meeting chat muted "until I join".
        match meeting_chat.filter(|_| self.call_in_progress()) {
            Some(thread) => Task::batch([task, self.joined_meeting_chat(&thread)]),
            None => task,
        }
    }
    fn start_call(&mut self, kind: CallKind, title: String, video: bool, muted: bool) -> Task<Msg> {
        self.call.error = None;
        if self.call_in_progress() {
            self.call.error = Some(t("core.call.a_call_is_already_in_progress").into());
            return Task::none();
        }
        if self.calls.surl.is_none() {
            self.call.error = Some(t("core.call.the_calls_channel_is_not_ready").into());
            return Task::none();
        }
        let Some(client) = self.client.clone() else {
            return Task::none();
        };
        let id = uuid::Uuid::new_v4().to_string();
        self.call.active = Some(ActiveCall::new(id.clone(), title, kind, video, muted));
        if let Some(ActiveCall {
            kind:
                CallKind::EchoTest {
                    bot,
                    chat_thread: None,
                },
            ..
        }) = &self.call.active
        {
            let g = self.generation;
            let me = self.me.clone();
            let person = teams_chat::Person {
                mri: bot.id.clone(),
                display_name: "Echo Bot".into(),
                friendly_name: String::new(),
                email: String::new(),
                role: None,
            };
            return Task::perform(
                async move {
                    client
                        .create_chat(&me, &[person], "")
                        .await
                        .map_err(|e| e.to_string())
                },
                move |result| Msg::Call(CallMsg::EchoChat(g, id, result)),
            );
        }
        self.fetch_relays(client, id)
    }

    /// TURN credentials for a call, then `CallMsg::Relays`.
    fn fetch_relays(&self, client: Client, id: String) -> Task<Msg> {
        let g = self.generation;
        let trap = TrapClient::new(client.authenticator());
        Task::perform(
            async move {
                // Without TURN credentials, STUN may still work outside corporate networks.
                let credentials = match trap.credentials().await {
                    Ok(c) => {
                        teams_calling::diagnostics::record("TRAP credentials received");
                        Some(c)
                    }
                    Err(e) => {
                        teams_calling::diagnostics::record(&format!("TRAP failed: {e}"));
                        None
                    }
                };
                trap::ice_servers(credentials.as_ref(), trap::DEFAULT_RELAY)
                    .into_iter()
                    .map(|s| IceServerView {
                        urls: s.urls,
                        username: s.username,
                        credential: s.credential,
                    })
                    .collect()
            },
            move |servers| Msg::Call(CallMsg::Relays(g, id, servers)),
        )
    }

    fn local(&self) -> Local {
        Local {
            id: self.me.clone(),
            display_name: self.display_name.clone(),
            endpoint_id: self.calls.push.endpoint.clone(),
            participant_id: uuid::Uuid::new_v4().to_string(),
            language_id: "fr-fr".into(),
        }
    }

    /// Runs the commands of the session, then closes media if the call ended.
    pub(crate) fn step(&mut self, input: Input) -> Task<Msg> {
        let g = self.generation;
        let Some(call) = self.call.active.as_mut() else {
            return Task::none();
        };
        let Some(session) = call.session.as_mut() else {
            return Task::none();
        };
        let mut tasks = Vec::new();
        let input_kind = match &input {
            Input::Callback { scope, name, body } => format!(
                "callback {scope:?}/{name} keys={:?} code={} subCode={}",
                body.as_object().map(|o| o.keys().collect::<Vec<_>>()),
                body.get("code").unwrap_or(&Value::Null),
                body.get("subCode").unwrap_or(&Value::Null),
            ),
            Input::Response(op, result) => {
                format!("response {op:?} status={:?}", result.as_ref().err())
            }
            _ => "local event".into(),
        };
        // End and rejection bodies explain failures (`debugContent`, phrase); `controlVideoStreaming` what it wants of our video.
        if let Input::Callback { name, body, .. } = &input
            && matches!(
                name.as_str(),
                "end" | "conversationEnd" | "mediaRejection" | "controlVideoStreaming"
            )
        {
            let text: String = body.to_string().chars().take(3000).collect();
            teams_calling::diagnostics::record(&format!("{name} body {text}"));
        }
        if let Input::Callback { name, body, .. } = &input
            && name == "rosterUpdate"
        {
            teams_calling::diagnostics::record(&teams_calling::diagnostics::roster_shape(body));
        }
        let commands = session.handle(input);
        teams_calling::diagnostics::record(&format!("{input_kind} phase={:?}", session.phase));
        for command in commands {
            let id = call.id.clone();
            match command {
                command @ (Command::Http { .. } | Command::Timer(..)) => {
                    tasks.extend(session_task(self.client.as_ref(), g, id, command));
                }
                // An incoming call's offer waits for the user to answer.
                Command::RemoteMedia(blob) if matches!(call.kind, CallKind::Incoming(_)) => {
                    call.remote_offer = Some(sdp::from_ngc(&blob, sdp::Kind::Offer, None));
                    call.remote_ngc_offer = Some(blob);
                }
                Command::SendLimits(limits) => {
                    tasks.push(Task::effect(Effect::MediaSendLimits {
                        call_id: id,
                        mid: webrtc_mid(
                            call.remote_ngc_offer.as_deref(),
                            call.local_sdp.as_deref(),
                            limits.mid,
                        ),
                        sharing: limits.sharing,
                        max_fs: limits.max_fs,
                        max_fps: limits.max_fps,
                        max_bitrate: limits.max_bitrate,
                        keyframe: limits.keyframe,
                    }));
                }
                Command::RemoteOffer(blob) => {
                    teams_calling::diagnostics::record(&format!(
                        "NGC renegotiation offer {}",
                        teams_calling::diagnostics::sdp_shape(&blob)
                    ));
                    tasks.push(Task::effect(Effect::MediaRemoteOffer {
                        call_id: id,
                        sdp: sdp::from_ngc(&blob, sdp::Kind::Offer, call.local_sdp.as_deref()),
                    }));
                    call.remote_ngc_offer = Some(blob);
                }
                Command::LocalOfferRejected { retry } => {
                    tasks.push(Task::effect(Effect::MediaOfferRejected {
                        call_id: id,
                        retry,
                    }));
                }
                Command::RemoteProvisionalMedia(blob) => {
                    tasks.push(Task::effect(Effect::MediaRemoteProvisionalDescription {
                        call_id: id,
                        sdp: sdp::from_ngc(&blob, sdp::Kind::Answer, call.local_sdp.as_deref()),
                    }))
                }
                Command::RemoteMedia(blob) => {
                    teams_calling::diagnostics::record(&format!(
                        "final remote SDP bytes={}",
                        blob.len()
                    ));
                    teams_calling::diagnostics::record(&format!(
                        "NGC answer {}",
                        teams_calling::diagnostics::sdp_shape(&blob)
                    ));
                    let answer = sdp::from_ngc(&blob, sdp::Kind::Answer, call.local_sdp.as_deref());
                    teams_calling::diagnostics::record(&format!(
                        "WebRTC answer {}",
                        teams_calling::diagnostics::sdp_shape(&answer)
                    ));
                    tasks.push(Task::effect(Effect::MediaRemoteDescription {
                        call_id: id,
                        sdp: answer,
                    }))
                }
            }
        }
        if session.phase == Phase::Connected && call.connected_at.is_none() {
            call.connected_at = Some(std::time::SystemTime::now());
        }
        if let Phase::Ended(reason) = &session.phase
            && call.ended.is_none()
        {
            call.ended = Some(reason.clone());
            tasks.push(Task::effect(Effect::MediaClose {
                call_id: call.id.clone(),
            }));
        }
        Task::batch(tasks)
    }

    fn end_locally(&mut self, reason: EndReason) -> Task<Msg> {
        let Some(call) = self.call.active.as_mut() else {
            return Task::none();
        };
        if call.ended.is_some() {
            return Task::none();
        }
        call.ended = Some(reason);
        Task::effect(Effect::MediaClose {
            call_id: call.id.clone(),
        })
    }

    fn step_watch(&mut self, input: Input) -> Task<Msg> {
        let g = self.generation;
        let Some(watch) = self.call.prejoin.as_mut().and_then(|p| p.watch.as_mut()) else {
            return Task::none();
        };
        let id = watch.id.clone();
        let commands = watch.handle(input);
        let client = self.client.as_ref();
        Task::batch(
            commands
                .into_iter()
                .filter_map(|c| session_task(client, g, id.clone(), c))
                .collect::<Vec<_>>(),
        )
    }

    fn watch_id(&self) -> Option<&str> {
        let watch = self.call.prejoin.as_ref()?.watch.as_ref()?;
        Some(&watch.id)
    }

    /// Leaves the pre-join roster subscription; its answers are no longer listened to.
    fn stop_watch(&mut self) -> Task<Msg> {
        let task = self.step_watch(Input::Hangup);
        if let Some(prejoin) = self.call.prejoin.as_mut() {
            prejoin.watch = None;
        }
        task
    }

    /// A request pushed through Trouter: a callback of the active call, or nothing yet.
    pub(crate) fn call_pushed(&mut self, request: &teams_realtime::PushedRequest) -> Task<Msg> {
        teams_calling::diagnostics::record(&format!(
            "push routed={} callAgent={}",
            self.call.active.as_ref().is_some_and(|c| {
                teams_calling::callbacks::CallbackPath::parse(&request.path)
                    .is_some_and(|p| p.call == c.id)
            }),
            request.path.contains("callAgent")
        ));
        if let Some(id) = self.watch_id().map(str::to_owned)
            && let Some(input) =
                teams_calling::driver::route(&id, &request.path, &request.headers, &request.body)
        {
            return self.step_watch(input);
        }
        if let Some(id) = self.call.active.as_ref().map(|c| c.id.clone())
            && let Some(input) =
                teams_calling::driver::route(&id, &request.path, &request.headers, &request.body)
        {
            return self.step(input);
        }
        let incoming = teams_calling::callbacks::decode_body(&request.body, &request.headers)
            .as_ref()
            .and_then(teams_calling::callbacks::incoming_call);
        match incoming {
            Some(incoming) => self.ring(incoming),
            None => Task::none(),
        }
    }

    /// Someone calls: attach and show it; while busy it is ignored, so it times out or goes to voicemail.
    fn ring(&mut self, incoming: teams_calling::callbacks::IncomingCall) -> Task<Msg> {
        if self.call.prejoin.is_some() || self.call_in_progress() {
            return Task::none();
        }
        let Some(surl) = self.calls.surl.clone() else {
            return Task::none();
        };
        let service = self
            .conversation_client()
            .and_then(|c| c.conversation_service())
            .unwrap_or_default();
        let id = uuid::Uuid::new_v4().to_string();
        let title = incoming
            .caller_name
            .clone()
            .filter(|n| !n.trim().is_empty())
            .or_else(|| self.known_name(&incoming.caller_id))
            .or_else(|| incoming.caller_id.strip_prefix("4:").map(str::to_owned))
            .unwrap_or_else(|| t("core.call.unknown_caller").into());
        let kind = CallKind::Incoming(incoming);
        let session = CallSession::new(
            id.clone(),
            self.local(),
            Callbacks::new(&surl, &self.calls.session, &id),
            kind.clone(),
            None,
            service,
        );
        self.call.error = None;
        self.call.active = Some(ActiveCall {
            session: Some(session),
            ..ActiveCall::new(id, title, kind, false, false)
        });
        self.step(Input::Begin)
    }

    fn is_active(&self, g: u64, id: &str) -> bool {
        g == self.generation && self.call.active.as_ref().is_some_and(|c| c.id == id)
    }

    /// Routes a session answer to the pre-join watch or the active call it belongs to.
    fn session_input(&mut self, g: u64, id: &str, input: Input) -> Task<Msg> {
        if g == self.generation && self.watch_id() == Some(id) {
            return self.step_watch(input);
        }
        if !self.is_active(g, id) {
            return Task::none();
        }
        self.step(input)
    }

    pub(crate) fn update_call(&mut self, msg: CallMsg) -> Task<Msg> {
        match msg {
            CallMsg::EchoChat(g, id, result) => {
                if !self.is_active(g, &id)
                    || self.call.active.as_ref().is_none_or(|c| c.ended.is_some())
                {
                    return Task::none();
                }
                match result {
                    Ok(thread) => {
                        let state = &mut *self.call;
                        let Some(call) = state.active.as_mut() else {
                            return Task::none();
                        };
                        if let CallKind::EchoTest { bot, chat_thread } = &mut call.kind {
                            state.echo_chat = Some((bot.id.clone(), thread.clone()));
                            *chat_thread = Some(thread);
                        }
                        teams_calling::diagnostics::record("Echo Bot conversation resolved");
                        match self.client.clone() {
                            Some(client) => self.fetch_relays(client, id),
                            None => Task::none(),
                        }
                    }
                    Err(error) => {
                        teams_calling::diagnostics::record(&format!(
                            "Echo Bot conversation failed: {error}"
                        ));
                        self.call.error = Some(tf!(
                            "core.call.could_not_prepare_the_echo_bot",
                            error = error
                        ));
                        self.end_locally(EndReason {
                            code: 480,
                            sub_code: 0,
                            phrase: teams_calling::session::ECHO_BOT_UNAVAILABLE.into(),
                            local: true,
                        })
                    }
                }
            }
            CallMsg::Relays(g, id, servers) => {
                if !self.is_active(g, &id) {
                    return Task::none();
                }
                let (video, muted, speaker, remote_offer) = self
                    .call
                    .active
                    .as_ref()
                    .map(|c| (c.video, c.muted, c.speaker, c.remote_offer.clone()))
                    .unwrap_or_default();
                let group = self.call.active.as_ref().is_some_and(ActiveCall::group);
                Task::effect(Effect::MediaPrepare {
                    call_id: id,
                    ice_servers: servers,
                    video,
                    muted,
                    speaker,
                    camera_lines: if group { CAMERA_LINES } else { 1 },
                    remote_offer,
                })
            }
            CallMsg::Http(g, id, op, result) => {
                self.session_input(g, &id, Input::Response(op, result))
            }
            CallMsg::Timer(g, id, timer) => self.session_input(g, &id, Input::Timeout(timer)),
            CallMsg::RecorderToken(g, id, result) => self.recorder_token(g, &id, result),
            CallMsg::Action(action) => self.call_action(action),
        }
    }

    fn call_action(&mut self, action: CallAction) -> Task<Msg> {
        match action {
            CallAction::StartRecording => self.start_recording(),
            CallAction::StopRecording => {
                if let Some(call) = self.call.active.as_mut() {
                    call.recording_error = None;
                }
                self.step(Input::StopRecording)
            }
            CallAction::Start { mri, video } => {
                let mri = mri.trim().to_owned();
                if !(mri.starts_with("8:") || mri.starts_with("4:") || mri.starts_with("28:")) {
                    self.call.error = Some(t("core.call.invalid_id_to_call_8_orgid").into());
                    return Task::none();
                }
                let title = self.known_name(&mri).unwrap_or_else(|| mri.clone());
                self.begin_call(
                    CallKind::Outgoing {
                        to: vec![Target {
                            id: mri,
                            display_name: None,
                        }],
                        chat_thread: None,
                    },
                    title,
                    video,
                    false,
                )
            }
            CallAction::CallChat { chat_id, video } => {
                let id = match (chat_id, self.selected.clone()) {
                    (Some(id), _) => id,
                    (None, Some(Selection::Chat(id))) => id,
                    _ => return Task::none(),
                };
                match self.chat_call(&id) {
                    Ok((kind, title)) => self.begin_call(kind, title, video, false),
                    Err(error) => {
                        self.call.error = Some(error);
                        Task::none()
                    }
                }
            }
            CallAction::JoinMeeting { url, video } => match self.meeting_link(&url) {
                Some(meeting) => self.begin_call(
                    CallKind::Meeting(meeting),
                    t("core.call.meeting").into(),
                    video,
                    false,
                ),
                None => Task::none(),
            },
            CallAction::PrepareMeeting { url, title } => match self.meeting_link(&url) {
                Some(meeting) => self.prepare(meeting, title),
                None => Task::none(),
            },
            CallAction::PrepareChatMeeting => {
                let Some(Selection::Chat(id)) = self.selected.clone() else {
                    return Task::none();
                };
                match self.chat_meeting(&id) {
                    Some(meeting) => {
                        let title = self.title();
                        self.prepare(meeting, title)
                    }
                    None => {
                        self.call.error = Some(t("core.call.this_chat_is_not_linked_to").into());
                        Task::none()
                    }
                }
            }
            CallAction::ConfirmJoin {
                video,
                muted,
                speaker,
                transfer,
            } => {
                let leave = self.stop_watch();
                let Some(prejoin) = self.call.prejoin.take() else {
                    return leave;
                };
                let join = self.begin_call(prejoin.kind, prejoin.title, video, muted);
                if let Some(call) = self.call.active.as_mut().filter(|c| c.ended.is_none()) {
                    call.speaker = speaker;
                    call.replace_other_devices = transfer;
                }
                Task::batch([leave, join])
            }
            CallAction::CancelJoin => {
                let leave = self.stop_watch();
                self.call.prejoin = None;
                self.call.error = None;
                leave
            }
            CallAction::SetCamera { on } => {
                // Also mid-call without a camera at the start: the engine renegotiates.
                let Some(call) = self.call.active.as_mut().filter(|c| c.ended.is_none()) else {
                    return Task::none();
                };
                call.camera = on;
                call.video |= on;
                Task::effect(Effect::MediaSetCamera {
                    call_id: call.id.clone(),
                    on,
                })
            }
            CallAction::ScreenShareStopped { call_id, reason } => {
                if let Some(call) = self.call.active.as_mut().filter(|c| c.id == call_id) {
                    call.screen = false;
                    self.call.error = reason;
                }
                Task::none()
            }
            CallAction::CameraStopped { call_id, reason } => {
                if let Some(call) = self.call.active.as_mut().filter(|c| c.id == call_id) {
                    call.camera = false;
                    self.call.error = reason;
                }
                Task::none()
            }
            CallAction::SetScreenShare { on } => {
                let Some(call) = self
                    .call
                    .active
                    .as_mut()
                    .filter(|c| c.in_phase(Phase::Connected))
                else {
                    return Task::none();
                };
                call.screen = on;
                Task::effect(Effect::MediaSetScreenShare {
                    call_id: call.id.clone(),
                    on,
                })
            }
            CallAction::SetSpeaker { on } => {
                let Some(call) = self.call.active.as_mut().filter(|c| c.ended.is_none()) else {
                    return Task::none();
                };
                call.speaker = on;
                Task::effect(Effect::MediaSetSpeaker {
                    call_id: call.id.clone(),
                    on,
                })
            }
            CallAction::LocalDescription {
                call_id,
                sdp: local,
            } => {
                teams_calling::diagnostics::record(&format!(
                    "WebRTC local {}",
                    teams_calling::diagnostics::sdp_shape(&local)
                ));
                // Our answer to an incoming call.
                if let Some(call) = self.call.active.as_mut().filter(|c| {
                    c.id == call_id && c.remote_offer.is_some() && c.in_phase(Phase::Notified)
                }) {
                    let media = Media {
                        video: call.media_video(),
                        screen: false,
                        content: payload::media_content(&match &call.remote_ngc_offer {
                            Some(offer) => sdp::answer_to_ngc(&local, offer, call.conference()),
                            None => sdp::to_ngc_in(&local, sdp::Kind::Answer, call.conference()),
                        }),
                    };
                    call.local_sdp = Some(local);
                    return self.step(Input::Accept(media));
                }
                // A new offer during the call: ICE restart after a network change.
                if let Some(call) = self
                    .call
                    .active
                    .as_mut()
                    .filter(|c| c.id == call_id && c.in_phase(Phase::Connected))
                {
                    let media = Media {
                        video: call.media_video(),
                        screen: call.screen,
                        content: payload::media_content(&sdp::to_ngc_in(
                            &local,
                            sdp::Kind::Offer,
                            call.conference(),
                        )),
                    };
                    call.local_sdp = Some(local);
                    return self.step(Input::Renegotiate(media));
                }
                let Some(call) = self
                    .call
                    .active
                    .as_ref()
                    .filter(|c| c.id == call_id && c.ended.is_none() && c.session.is_none())
                else {
                    return Task::none();
                };
                let (Some(surl), Some(service)) = (
                    self.calls.surl.clone(),
                    self.conversation_client()
                        .and_then(|c| c.conversation_service()),
                ) else {
                    return self.end_locally(EndReason {
                        code: 480,
                        sub_code: 0,
                        phrase: "NoPushChannel".into(),
                        local: true,
                    });
                };
                let media = Media {
                    video: call.media_video(),
                    screen: false,
                    content: payload::media_content(&sdp::to_ngc_in(
                        &local,
                        sdp::Kind::Offer,
                        call.conference(),
                    )),
                };
                let mut session = CallSession::new(
                    call_id.clone(),
                    self.local(),
                    Callbacks::new(&surl, &self.calls.session, &call_id),
                    call.kind.clone(),
                    Some(media),
                    service,
                );
                session.replace_other_devices = call.replace_other_devices;
                let Some(active) = self.call.active.as_mut() else {
                    return Task::none();
                };
                active.session = Some(session);
                active.local_sdp = Some(local);
                self.step(Input::Begin)
            }
            CallAction::MediaConnected { call_id } => {
                if let Some(call) = self.call.active.as_mut().filter(|c| c.id == call_id) {
                    call.media_connected = true;
                }
                Task::none()
            }
            CallAction::MediaFailed { call_id, reason } => {
                if self.call.active.as_ref().is_none_or(|c| c.id != call_id) {
                    return Task::none();
                }
                self.call.error = Some(reason);
                self.hang_up()
            }
            CallAction::Accept { video } => {
                let Some(call) = self
                    .call
                    .active
                    .as_mut()
                    .filter(|c| c.remote_offer.is_some() && c.in_phase(Phase::Notified))
                else {
                    return Task::none();
                };
                call.video = video;
                call.camera = video;
                let id = call.id.clone();
                match self.client.clone() {
                    Some(client) => self.fetch_relays(client, id),
                    None => Task::none(),
                }
            }
            CallAction::Decline => self.step(Input::Decline),
            CallAction::TestCall { bot_mri } => {
                teams_calling::diagnostics::record("starting Echo Bot test");
                let bot = bot_mri
                    .map(|m| m.trim().to_owned())
                    .filter(|m| m.starts_with("28:"))
                    .unwrap_or_else(echo_bot_mri);
                // The web client calls within the chat with the bot; without one, ring the bot.
                let chat_thread = self
                    .chats
                    .iter()
                    .find(|c| c.members.iter().any(|m| m.id.eq_ignore_ascii_case(&bot)))
                    .map(|c| c.id.clone())
                    .or_else(|| {
                        self.call
                            .echo_chat
                            .as_ref()
                            .filter(|(mri, _)| mri == &bot)
                            .map(|(_, id)| id.clone())
                    });
                let kind = CallKind::EchoTest {
                    bot: Target {
                        id: bot,
                        display_name: Some("Echo Bot".into()),
                    },
                    chat_thread,
                };
                self.begin_call(kind, t("core.call.test_call").into(), true, false)
            }
            #[cfg(not(feature = "demo"))]
            CallAction::SimulateIncoming { .. } => Task::none(),
            #[cfg(feature = "demo")]
            CallAction::SimulateIncoming { video } => {
                let demo = self
                    .client
                    .as_ref()
                    .is_some_and(|c| matches!(*c.authenticator(), AppAuthenticator::Demo(_)));
                if demo {
                    crate::calls_mock::conversation().ring_me(
                        "8:orgid:alice",
                        "Alice Martin",
                        video,
                    );
                }
                Task::none()
            }
            CallAction::RenegotiationAnswer {
                call_id,
                sdp: answer,
            } => {
                let Some(call) = self
                    .call
                    .active
                    .as_mut()
                    .filter(|c| c.id == call_id && c.ended.is_none())
                else {
                    return Task::none();
                };
                let ngc = match &call.remote_ngc_offer {
                    Some(offer) => sdp::answer_to_ngc(&answer, offer, call.conference()),
                    None => sdp::to_ngc_in(&answer, sdp::Kind::Answer, call.conference()),
                };
                teams_calling::diagnostics::record(&format!(
                    "NGC renegotiation answer {}",
                    teams_calling::diagnostics::sdp_shape(&ngc)
                ));
                let media = Media {
                    video: call.media_video(),
                    screen: call.screen,
                    content: payload::media_content(&ngc),
                };
                self.step(Input::RenegotiationAnswer(media))
            }
            CallAction::NetworkChanged => {
                // The old socket is likely dead: reconnect at once rather than wait for pings.
                let restart = self.restart_realtime();
                let ice = self
                    .call
                    .active
                    .as_ref()
                    .filter(|c| c.in_phase(Phase::Connected))
                    .map(|c| {
                        Task::effect(Effect::MediaRestartIce {
                            call_id: c.id.clone(),
                        })
                    })
                    .unwrap_or_else(Task::none);
                Task::batch([restart, ice])
            }
            CallAction::Hangup => self.hang_up(),
            CallAction::SetMuted { muted } => {
                let Some(call) = self.call.active.as_mut().filter(|c| c.ended.is_none()) else {
                    return Task::none();
                };
                call.muted = muted;
                let effect = Task::effect(Effect::MediaSetMuted {
                    call_id: call.id.clone(),
                    muted,
                });
                // Local mute is enough in a 1:1 call; meetings also learn it server-side.
                Task::batch([effect, self.step(Input::SetMuted(muted))])
            }
            CallAction::Dismiss => {
                if self.call.active.as_ref().is_some_and(|c| c.ended.is_some()) {
                    self.call.active = None;
                }
                Task::none()
            }
        }
    }

    fn meeting_link(&mut self, url: &str) -> Option<teams_calling::MeetingCoordinates> {
        match MeetingLink::parse(url) {
            Some(MeetingLink::Coordinates(meeting)) => Some(meeting),
            Some(MeetingLink::Code { .. }) => {
                self.call.error = Some(t("core.call.meet_meeting_links_are_not_supported").into());
                None
            }
            None => {
                self.call.error = Some(t("core.call.unrecognized_teams_meeting_link").into());
                None
            }
        }
    }

    fn prepare(&mut self, meeting: teams_calling::MeetingCoordinates, title: String) -> Task<Msg> {
        self.call.error = None;
        if self.call_in_progress() {
            self.call.error = Some(t("core.call.a_call_is_already_in_progress").into());
            return Task::none();
        }
        let leave = self.stop_watch();
        self.call.active = None;
        let kind = CallKind::Meeting(meeting);
        // The web client's pre-join subscription, from an endpoint of its own.
        let watch = match (
            self.calls.surl.clone(),
            self.conversation_client()
                .and_then(|c| c.conversation_service()),
        ) {
            (Some(surl), Some(service)) => {
                let id = uuid::Uuid::new_v4().to_string();
                let local = Local {
                    endpoint_id: uuid::Uuid::new_v4().to_string(),
                    ..self.local()
                };
                let callbacks = Callbacks::new(&surl, &self.calls.session, &id);
                Some(CallSession::new(
                    id,
                    local,
                    callbacks,
                    kind.clone(),
                    None,
                    service,
                ))
            }
            _ => None,
        };
        self.call.prejoin = Some(Prejoin { kind, title, watch });
        Task::batch([leave, self.step_watch(Input::Begin)])
    }

    /// Join coordinates of a meeting chat: its `meetingInformation`, or the thread itself.
    pub(crate) fn chat_meeting(&self, id: &str) -> Option<teams_calling::MeetingCoordinates> {
        let chat = self.chats.iter().find(|c| c.id == id);
        let info = chat.and_then(|c| c.extra.get("meetingInformation"));
        let field = |key: &str| {
            info.and_then(|i| i.get(key))
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        };
        if let Some(Some(MeetingLink::Coordinates(meeting))) =
            field("meetingJoinUrl").map(|url| MeetingLink::parse(&url))
        {
            return Some(meeting);
        }
        id.starts_with("19:meeting_")
            .then(|| teams_calling::MeetingCoordinates {
                thread_id: id.to_owned(),
                message_id: "0".into(),
                tenant_id: field("tenantId"),
                organizer_id: field("organizerId"),
            })
    }

    /// Who a call from this chat rings: every other member; the chat ties a group call to it.
    pub(crate) fn chat_call(&self, id: &str) -> Result<(CallKind, String), String> {
        let members = self.chat_members(id).ok_or(t("core.call.chat_not_found"))?;
        if self.chat_meeting(id).is_some() {
            return Err(t("core.call.this_is_a_meeting_chat_use").into());
        }
        let mut seen = HashSet::new();
        let to: Vec<Target> = members
            .iter()
            .filter(|m| !m.id.eq_ignore_ascii_case(&self.me))
            .filter(|m| ["8:", "4:"].iter().any(|p| m.id.starts_with(p)))
            .filter(|m| seen.insert(m.id.to_ascii_lowercase()))
            .map(|m| Target {
                id: m.id.clone(),
                display_name: self.known_name(&m.id),
            })
            .collect();
        if to.is_empty() {
            return Err(t("core.call.no_one_to_call_in_this").into());
        }
        if to.len() > MAX_GROUP_CALL {
            return Err(tf!(
                "core.call.too_many_participants_for_a_group",
                MAX_GROUP_CALL = MAX_GROUP_CALL
            ));
        }
        Ok((
            CallKind::Outgoing {
                to,
                chat_thread: Some(id.to_owned()).filter(|id| id.starts_with("19:")),
            },
            self.chats
                .iter()
                .find(|c| c.id == id)
                .map(|c| chat_title(c, &self.me))
                .unwrap_or_else(|| t("core.call.chat").into()),
        ))
    }

    /// A chat's members: its looked-up roster when there is one.
    pub(crate) fn chat_members(&self, id: &str) -> Option<&[teams_chat::Member]> {
        self.details
            .get(id)
            .map(|d| d.members.as_slice())
            .or_else(|| {
                self.chats
                    .iter()
                    .find(|c| c.id == id)
                    .map(|c| c.members.as_slice())
            })
    }

    /// The push channel reconnected elsewhere: move the active call's callbacks.
    pub(crate) fn call_moved(&mut self, surl: String) -> Task<Msg> {
        let session = self.calls.session.clone();
        if self.call_in_progress() {
            self.step(Input::Rebase { surl, session })
        } else {
            Task::none()
        }
    }

    /// A call is ringing, connecting or connected.
    pub(crate) fn call_in_progress(&self) -> bool {
        self.call.active.as_ref().is_some_and(|c| c.ended.is_none())
    }

    fn hang_up(&mut self) -> Task<Msg> {
        match self.call.active.as_ref() {
            // Hanging up while it rings declines.
            Some(call) if call.in_phase(Phase::Notified) => self.step(Input::Decline),
            Some(call) if call.session.is_some() && call.ended.is_none() => {
                self.step(Input::Hangup)
            }
            Some(_) => self.end_locally(EndReason {
                code: 0,
                sub_code: 0,
                phrase: "LocalUserEnded".into(),
                local: true,
            }),
            None => Task::none(),
        }
    }
}

#[cfg(test)]
#[path = "call/tests.rs"]
mod tests;

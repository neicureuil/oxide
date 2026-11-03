//! One call or meeting as a pure state machine (inputs → commands, no I/O), so tests can replay any sequence.
use std::collections::BTreeMap;
use std::time::Duration;

use http::Method;
use serde_json::Value;
use teams_i18n::{t, tf};

use crate::MeetingCoordinates;
use crate::callbacks::{IncomingCall, Scope};
use crate::payload::{self, Callbacks, Local, Media, Target};

/// Delays of `csaTimeoutConfiguration`.
pub const ESTABLISH_TIMEOUT: Duration = Duration::from_secs(90);
pub const INCOMING_TIMEOUT: Duration = Duration::from_secs(75);
/// `conversationKeepAliveTimeoutSec` (2700 s), renewed a little early.
pub const KEEP_ALIVE: Duration = Duration::from_secs(2400);
/// End phrase of an incoming call nobody answered here.
pub const MISSED: &str = "Missed";
/// End phrase of a test call the Echo Bot could not join.
pub const ECHO_BOT_UNAVAILABLE: &str = "EchoBotUnavailable";
/// `CALL_END_PHRASE.LOCAL_USER_INITIATED`, with code 487 (CANCEL) for an unanswered call.
const LOCAL_USER_INITIATED: &str = "CallEndReasonLocalUserInitiated";
/// New offers after the service rejects ours, like the web client's `renegotiationAttempts`.
const RENEGOTIATION_RETRIES: u32 = 3;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallKind {
    /// Scheduled meeting or existing group call; `media: None` only follows the roster.
    Meeting(MeetingCoordinates),
    Outgoing {
        to: Vec<Target>,
        chat_thread: Option<String>,
    },
    Incoming(IncomingCall),
    /// Echo Bot test: a call tied to the (already resolved) bot chat that rings nobody; the bot is added once accepted.
    EchoTest {
        bot: Target,
        chat_thread: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EndReason {
    pub code: u16,
    pub sub_code: u32,
    pub phrase: String,
    /// Ended from this side (hang up, decline, timeout).
    pub local: bool,
}

impl EndReason {
    fn local(code: u16, phrase: &str) -> Self {
        Self {
            code,
            sub_code: 0,
            phrase: phrase.into(),
            local: true,
        }
    }

    fn from_body(body: &Value) -> Self {
        let end = ["callEnd", "conversationEnd", "end"]
            .iter()
            .find_map(|k| body.get(*k))
            .unwrap_or(body);
        Self {
            code: end.get("code").and_then(Value::as_u64).unwrap_or(0) as u16,
            sub_code: end.get("subCode").and_then(Value::as_u64).unwrap_or(0) as u32,
            phrase: end
                .get("phrase")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
            local: false,
        }
    }

    /// Explanation for the UI, in the app language (`CALL_END_CODE`).
    pub fn message(&self) -> String {
        if self.phrase == MISSED {
            return t("calling.session.missed_call").into();
        }
        if self.phrase == ECHO_BOT_UNAVAILABLE {
            return t("calling.session.echo_bot_couldn_t_join_the")
            .into();
        }
        match self.code {
            0 if self.local => t("calling.session.you_left_the_call").into(),
            0 => t("calling.session.the_call_has_ended").into(),
            403 => t("calling.session.access_to_the_call_was_denied").into(),
            404 | 481 => t("calling.session.call_or_meeting_not_found")
            .into(),
            408 => t("calling.session.no_one_answered_in_time").into(),
            409 => t("calling.session.conflict_the_call_was_picked_up")
            .into(),
            410 => t("calling.session.audio_video_connection_failed")
            .into(),
            413 => t("calling.session.the_meeting_has_reached_its_maximum")
            .into(),
            480 => t("calling.session.the_person_you_called_isn_t")
            .into(),
            486 => t("calling.session.the_person_you_called_is_busy")
            .into(),
            487 => t("calling.session.call_canceled").into(),
            490 => t("calling.session.network_problem_during_the_call")
            .into(),
            495 => t("calling.session.your_teams_session_has_expired")
            .into(),
            603 => t("calling.session.call_declined").into(),
            code => tf!("calling.session.call_ended_code", code = code),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Phase {
    /// Incoming: ringing here, waiting for accept or decline.
    Notified,
    Connecting,
    /// Outgoing: the other side is ringing.
    Ringing,
    /// Meeting: waiting to be admitted.
    Lobby,
    Connected,
    Ending,
    Ended(EndReason),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RosterEntry {
    pub id: String,
    pub display_name: Option<String>,
    pub role: Option<String>,
    pub in_lobby: bool,
    /// Endpoints with the call modality (`endpoints.<id>.call`): devices in the call.
    pub call_endpoints: Vec<String>,
    /// `sourceId` of its audio streams: the CSRCs of its voice in the mixed audio.
    pub audio_sources: Vec<u32>,
    /// `sourceId` of its camera and of its shared screen, when sent.
    pub video_sources: Vec<u32>,
    pub sharing_sources: Vec<u32>,
    /// Microphone off on every device in the call.
    pub muted: bool,
    /// The cloud recording bot's endpoint metadata, when this is the bot.
    pub recorder: Option<Recorder>,
}

/// `processingModes.recording.state` of the recorder bot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecorderState {
    Inactive,
    Active,
    WaitingForMeetingLive,
    Paused,
}

/// The recorder bot as its roster entry describes it (`endpoints[].endpointMetadata`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recorder {
    /// Where `start` / `stop` go.
    pub command_url: Option<String>,
    pub state: RecorderState,
    /// Who started (or stopped) the recording: `processingModes.recording.userId`.
    pub user_id: Option<String>,
}

impl Recorder {
    /// `endpointMetadata` comes as an object or as a JSON string.
    fn parse(entry: &Value) -> Option<Self> {
        let endpoints = entry.get("endpoints")?;
        let list: Vec<&Value> = match endpoints {
            Value::Object(map) => map.values().collect(),
            Value::Array(list) => list.iter().collect(),
            _ => return None,
        };
        list.into_iter().find_map(|endpoint| {
            let metadata = match endpoint.get("endpointMetadata")? {
                Value::String(raw) => serde_json::from_str::<Value>(raw).ok()?,
                other => other.clone(),
            };
            let mode = metadata.pointer("/processingModes/recording");
            let state = match mode.and_then(|m| m.get("state")).and_then(Value::as_str) {
                Some("Active") => RecorderState::Active,
                Some("WaitingForMeetingLive") => RecorderState::WaitingForMeetingLive,
                Some("Paused") => RecorderState::Paused,
                _ => RecorderState::Inactive,
            };
            let command_url = text(metadata.get("commandUrl"))
                // `isTrustedCommandUrl`: https only (the configured domain list is empty).
                .filter(|url| url.starts_with("https://"));
            (command_url.is_some() || mode.is_some()).then(|| Recorder {
                command_url,
                state,
                user_id: text(mode.and_then(|m| m.get("userId"))),
            })
        })
    }
}

/// Where our own request to record stands; the bot's roster entry tells the rest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Recording {
    Idle,
    /// The bot is being added; `start` goes once it shows up in the roster.
    Inviting(payload::recording::Request),
    /// `start` sent: waiting for the bot to report `Active`.
    Starting,
    Stopping,
    /// The bot could not be added or refused the command.
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Join,
    Start,
    /// Join after the n-th 409 redirect; answers to earlier joins are stale.
    Redirected(u32),
    Attach,
    /// Ringing notice to the caller.
    Progress,
    Accept,
    AcceptAck,
    /// Our new offer during the call.
    Renegotiate,
    /// Our answer to the service's new offer.
    MediaAnswer,
    MediaAck,
    AddParticipant,
    Reject,
    Leave,
    /// Removes our other devices from the call (`removeParticipant` with `toEndpoints`).
    RemoveEndpoints,
    /// Asks the mixer for a participant's camera or screen on one of our video lines.
    SourceRequest,
    /// DELETE on the conversation controller: ends the call for everyone.
    EndForAll,
    Mute(bool),
    KeepAlive,
    /// Adding the recorder bot.
    RecorderInvite,
    /// `start` (true) or `stop` to the bot's `commandUrl`, with the recorder's AAD token.
    RecorderCommand(bool),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Timer {
    Establish,
    KeepAlive,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Input {
    /// Send the first request (join, start, or attach for an incoming call).
    Begin,
    Response(Op, Result<Value, u16>),
    Callback {
        scope: Scope,
        name: String,
        body: Value,
    },
    /// Answer an incoming call with our media answer.
    Accept(Media),
    /// A new local offer during the call (ICE restart after a network change).
    Renegotiate(Media),
    /// Our answer to a [`Command::RemoteOffer`].
    RenegotiationAnswer(Media),
    /// The push channel reconnected with a new session URL: callbacks move there.
    Rebase {
        surl: String,
        session: String,
    },
    Hangup,
    Decline,
    SetMuted(bool),
    /// Cloud recording: add the recorder bot (if needed) and tell it to start.
    StartRecording(payload::recording::Request),
    StopRecording,
    Timeout(Timer),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Http {
        op: Op,
        method: Method,
        url: String,
        body: Value,
    },
    Timer(Timer, Duration),
    /// NGC SDP of the other side (`mediaAnswer`, or the offer of an incoming call).
    RemoteMedia(String),
    /// Early media before acceptance; must not close the WebRTC offer/answer exchange.
    RemoteProvisionalMedia(String),
    /// The service renegotiates: answer this NGC offer with [`Input::RenegotiationAnswer`].
    RemoteOffer(String),
    /// What the mixer takes from one of our sending lines (`controlVideoStreaming`, as `setMaxCapabilities`).
    SendLimits(SendLimits),
    /// The service rejected our renegotiation: drop the local offer, then offer again or go back to
    /// what was last negotiated.
    LocalOfferRejected {
        retry: bool,
    },
}

/// Encoder bounds for one of our video lines, from the mixer's `fmtParams`.
#[derive(Debug, Clone, PartialEq)]
pub struct SendLimits {
    /// NGC mid of the line.
    pub mid: String,
    pub sharing: bool,
    /// Macroblocks per frame (`max-fs`).
    pub max_fs: u32,
    /// Frames per second (`max-fps` is in hundredths).
    pub max_fps: f64,
    /// Bits per second (`max-br` × 1200, as `parseFmtParams`).
    pub max_bitrate: u32,
    /// A keyframe now: asked (`KeyFrame=1`) or by default (`webrtcAllowRestoreKeyframe`).
    pub keyframe: bool,
}

pub struct CallSession {
    /// Call id: callback path segment and `X-Microsoft-Skype-Chain-ID`.
    pub id: String,
    pub local: Local,
    pub kind: CallKind,
    pub phase: Phase,
    pub roster: BTreeMap<String, RosterEntry>,
    /// Latest `version` seen per participant: delta rosters have their own ordering.
    roster_versions: BTreeMap<String, i64>,
    pub muted: bool,
    acknowledged: bool,
    echo_bot_added: bool,
    /// The mixer accepted our leg (`fromMixer`) but nobody called has joined yet.
    awaiting_answer: bool,
    /// The service's mixer took our leg (`mediaContent.fromMixer`): a hosted call.
    mixed: bool,
    /// Once in, our other devices leave the call: it moves to this one.
    pub replace_other_devices: bool,
    /// Video lines of the latest negotiation, and whose source each one was asked for.
    video_slots: Vec<crate::sdp::VideoSlot>,
    video_assigned: BTreeMap<String, (u32, String)>,
    video_sequence: u32,
    cameras_shown: usize,
    /// Where to send our answer to the service's pending renegotiation.
    pending_answer: Option<String>,
    /// Why we are leaving, reported once the leave request completes.
    leaving: Option<EndReason>,
    renegotiations: u32,
    /// Media before our pending renegotiation, restored if the service rejects it.
    offered_from: Option<Media>,
    rejected_offers: u32,
    /// `selfParticipant` of the latest join answer: consent and captcha requirements.
    pub self_participant: Option<Value>,
    callbacks: Callbacks,
    media: Option<Media>,
    /// URL the join is sent to: the conversation service, or a 409 redirect target.
    join_url: String,
    redirects: u32,
    controller: Option<String>,
    links: BTreeMap<String, String>,
    sequence: Option<i64>,
    pub recording: Recording,
    /// `meetingDetails` of the latest conversation update: capabilities such as `allowCloudRecording`.
    pub meeting_details: Option<Value>,
}

fn text(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
}

/// Lobby state, if the update says anything; the field names are unverified guesses around the web client's `lobby`.
fn lobby_state(body: &Value) -> Option<bool> {
    let candidates = [
        body.get("selfParticipant"),
        body.get("localParticipant"),
        body.get("participant"),
        body.get("conversationUpdate")
            .and_then(|c| c.get("selfParticipant")),
        Some(body),
    ];
    for candidate in candidates.into_iter().flatten() {
        if let Some(flag) = candidate.get("isInLobby").and_then(Value::as_bool) {
            return Some(flag);
        }
        for key in ["state", "lobbyState", "participantState"] {
            if let Some(state) = candidate.get(key).and_then(Value::as_str) {
                return Some(state.to_ascii_lowercase().contains("lobby"));
            }
        }
    }
    None
}

impl CallSession {
    pub fn new(
        id: String,
        local: Local,
        callbacks: Callbacks,
        kind: CallKind,
        media: Option<Media>,
        conversation_service: String,
    ) -> Self {
        let phase = match kind {
            CallKind::Incoming(_) => Phase::Notified,
            _ => Phase::Connecting,
        };
        Self {
            id,
            local,
            kind,
            phase,
            roster: BTreeMap::new(),
            roster_versions: BTreeMap::new(),
            muted: false,
            acknowledged: false,
            echo_bot_added: false,
            awaiting_answer: false,
            mixed: false,
            replace_other_devices: false,
            video_slots: Vec::new(),
            video_assigned: BTreeMap::new(),
            video_sequence: 0,
            cameras_shown: 0,
            pending_answer: None,
            leaving: None,
            renegotiations: 0,
            offered_from: None,
            rejected_offers: 0,
            self_participant: None,
            callbacks,
            media,
            join_url: conversation_service,
            redirects: 0,
            controller: None,
            links: BTreeMap::new(),
            sequence: None,
            recording: Recording::Idle,
            meeting_details: None,
        }
    }

    pub fn link(&self, name: &str) -> Option<&str> {
        self.links.get(name).map(String::as_str)
    }

    pub fn ended(&self) -> bool {
        matches!(self.phase, Phase::Ended(_))
    }

    /// The meeting asks attendees to consent (recording, transcription) before taking part.
    pub fn consent_required(&self) -> bool {
        self.self_participant
            .as_ref()
            .and_then(|me| me.get("enforceConsentToJoin"))
            .and_then(Value::as_bool)
            .unwrap_or(false)
    }

    /// Roster-only meeting join (no media).
    pub fn subscribed(&self) -> bool {
        self.media.is_none() && matches!(self.kind, CallKind::Meeting(_))
    }

    fn end(&mut self, reason: EndReason) -> Vec<Command> {
        self.phase = Phase::Ended(reason);
        Vec::new()
    }

    fn http(op: Op, method: Method, url: String, body: Value) -> Command {
        Command::Http {
            op,
            method,
            url,
            body,
        }
    }

    fn join_command(&self) -> Command {
        let body = match &self.kind {
            CallKind::Meeting(meeting) => {
                payload::join(&self.local, &self.callbacks, meeting, self.media.as_ref())
            }
            CallKind::Outgoing { to, chat_thread } => payload::start_call(
                &self.local,
                &self.callbacks,
                to,
                chat_thread.as_deref(),
                self.media
                    .as_ref()
                    .expect("checked at Begin: outgoing calls have a media offer"),
            ),
            CallKind::EchoTest { chat_thread, .. } => {
                // Tied to the bot chat, the call rings nobody: the bot is added afterwards.
                payload::join_group_call(
                    &self.local,
                    &self.callbacks,
                    chat_thread.as_deref(),
                    self.media
                        .as_ref()
                        .expect("checked at Begin: test calls have a media offer"),
                )
            }
            CallKind::Incoming(_) => unreachable!("incoming calls attach instead"),
        };
        let op = if self.redirects > 0 {
            Op::Redirected(self.redirects)
        } else if matches!(self.kind, CallKind::Outgoing { .. }) {
            Op::Start
        } else {
            Op::Join
        };
        Self::http(op, Method::POST, self.join_url.clone(), body)
    }

    /// Takes links, controller, roster and lobby state; `false` for an update older than the current `sequenceNumber`.
    fn apply(&mut self, body: &Value) -> bool {
        if let Some(sequence) = body.get("sequenceNumber").and_then(Value::as_i64) {
            if self.sequence.is_some_and(|current| sequence < current) {
                return false;
            }
            self.sequence = Some(sequence);
        }
        if let Some(me) = body.get("selfParticipant").filter(|v| v.is_object()) {
            self.self_participant = Some(me.clone());
        }
        if let Some(details) = body.get("meetingDetails").filter(|v| v.is_object()) {
            self.meeting_details = Some(details.clone());
        }
        if let Some(controller) = text(body.get("conversationController")) {
            self.controller = Some(controller);
        }
        for links in [
            body.get("links"),
            body.get("callAcceptance").and_then(|a| a.get("links")),
            body.pointer("/callAcceptanceAcknowledgement/links"),
            body.pointer("/callInvitation/links"),
            body.pointer("/mediaAcknowledgement/links"),
        ]
        .into_iter()
        .flatten()
        {
            if let Some(links) = links.as_object() {
                for (name, url) in links {
                    if let Some(url) = url.as_str() {
                        self.links.insert(name.clone(), url.to_owned());
                    }
                }
            }
        }
        if let Some(roster) = body.get("roster").or_else(|| body.get("rosterUpdate")) {
            self.apply_roster(roster);
        } else if body.get("participants").is_some() && body.get("conversationController").is_none()
        {
            self.apply_roster(body);
        }
        if let Some(lobby) = lobby_state(body) {
            match (&self.phase, lobby) {
                (Phase::Connecting | Phase::Connected, true) => self.phase = Phase::Lobby,
                (Phase::Lobby, false) => self.phase = Phase::Connected,
                _ => {}
            }
        }
        true
    }

    /// Delta roster: `participants` as an object keyed by MRI or as an array. Unverified shape.
    fn apply_roster(&mut self, roster: &Value) {
        let in_call = |value: &Value| -> Vec<Value> {
            value
                .get("endpoints")
                .and_then(Value::as_object)
                .map(|e| {
                    e.values()
                        .filter(|e| e.get("call").is_some())
                        .cloned()
                        .collect()
                })
                .unwrap_or_default()
        };
        let streams = |endpoint: &Value, kind: &dyn Fn(&str) -> bool| -> Vec<Value> {
            endpoint
                .pointer("/call/mediaStreams")
                .and_then(Value::as_array)
                .map(|streams| {
                    streams
                        .iter()
                        .filter(|m| m.get("type").and_then(Value::as_str).is_some_and(kind))
                        .cloned()
                        .collect()
                })
                .unwrap_or_default()
        };
        let audio = |endpoint: &Value| streams(endpoint, &|t| t == "audio");
        let source_ids = |value: &Value, kind: &dyn Fn(&str) -> bool| -> Vec<u32> {
            in_call(value)
                .iter()
                .flat_map(|e| streams(e, kind))
                // Streams the endpoint only receives carry no source to ask for.
                .filter(|m| m.get("direction").and_then(Value::as_str) != Some("recvonly"))
                .filter_map(|m| m.get("sourceId").and_then(Value::as_u64))
                .map(|id| id as u32)
                .collect()
        };
        let entry = |id: &str, value: &Value| RosterEntry {
            id: id.to_owned(),
            display_name: text(value.get("displayName"))
                .or_else(|| text(value.pointer("/details/displayName"))),
            role: text(value.get("role")).or_else(|| text(value.get("meetingRole"))),
            in_lobby: lobby_state(value).unwrap_or(false),
            call_endpoints: value
                .get("endpoints")
                .and_then(Value::as_object)
                .map(|e| {
                    e.iter()
                        .filter(|(_, e)| e.get("call").is_some())
                        .map(|(id, _)| id.clone())
                        .collect()
                })
                .unwrap_or_default(),
            audio_sources: in_call(value)
                .iter()
                .flat_map(audio)
                .filter_map(|m| m.get("sourceId").and_then(Value::as_u64))
                .map(|id| id as u32)
                .collect(),
            video_sources: source_ids(value, &|t| t == "video"),
            sharing_sources: source_ids(value, &|t| t.to_ascii_lowercase().contains("sharing")),
            muted: {
                let endpoints = in_call(value);
                !endpoints.is_empty()
                    && endpoints.iter().all(|e| {
                        e.pointer("/endpointState/state/isMuted")
                            .and_then(Value::as_bool)
                            == Some(true)
                            || audio(e).iter().any(|m| {
                                m.get("serverMuted").and_then(Value::as_bool) == Some(true)
                            })
                    })
            },
            recorder: payload::recording::is_bot(id)
                .then(|| Recorder::parse(value))
                .flatten(),
        };
        match roster.get("participants") {
            Some(Value::Object(map)) => {
                for (key, value) in map {
                    if let Some(version) = value.get("version").and_then(Value::as_i64) {
                        if self
                            .roster_versions
                            .get(key)
                            .is_some_and(|seen| version <= *seen)
                        {
                            continue;
                        }
                        self.roster_versions.insert(key.clone(), version);
                    }
                    // `fromRoster`: the person is `details.id`, not the key, which can name one of their endpoints.
                    let id = text(value.pointer("/details/id")).unwrap_or_else(|| key.clone());
                    // `fromRoster` returns null without a call, lobby, mapped or stream endpoint: not in the call.
                    let present = value.get("endpoints").and_then(Value::as_object).is_none_or(|e| {
                        e.values().any(|e| {
                            ["call", "lobby", "mappedParticipant", "stream", "streamLobby"]
                                .iter()
                                .any(|k| e.get(*k).is_some())
                        })
                    });
                    let gone = value.is_null()
                        || value.get("state").and_then(Value::as_str) == Some("inactive")
                        || !present;
                    if gone {
                        self.roster.remove(&id);
                    } else {
                        self.roster.insert(id.clone(), entry(&id, value));
                    }
                }
            }
            Some(Value::Array(list)) => {
                for value in list {
                    if let Some(id) = text(value.get("id")) {
                        self.roster.insert(id.clone(), entry(&id, value));
                    }
                }
            }
            _ => {}
        }
    }

    pub fn handle(&mut self, input: Input) -> Vec<Command> {
        if self.ended() {
            return Vec::new();
        }
        match input {
            Input::Begin => match &self.kind {
                CallKind::Incoming(call) => {
                    let controller = call.conversation_controller.clone().unwrap_or_default();
                    let body = payload::attach(&self.local, &self.callbacks, &controller, true);
                    vec![
                        Self::http(Op::Attach, Method::POST, call.attach.clone(), body),
                        Command::Timer(Timer::Establish, INCOMING_TIMEOUT),
                    ]
                }
                CallKind::Outgoing { .. } | CallKind::EchoTest { .. } if self.media.is_none() => {
                    self.end(EndReason::local(410, "NoLocalMedia"))
                }
                _ => {
                    let mut commands = vec![self.join_command()];
                    if !self.subscribed() {
                        commands.push(Command::Timer(Timer::Establish, ESTABLISH_TIMEOUT));
                    }
                    commands
                }
            },
            Input::Response(op, result) => self.response(op, result),
            Input::Callback { scope, name, body } => self.callback(scope, &name, body),
            Input::Hangup => self.hang_up(EndReason::local(0, "LocalUserEnded")),
            Input::Renegotiate(media) => match self.link("mediaRenegotiation").map(str::to_owned) {
                Some(url) if self.phase == Phase::Connected => {
                    self.renegotiations += 1;
                    let tag = format!("r{}", self.renegotiations);
                    let body = payload::renegotiation(&self.local, &self.callbacks, &media, &tag);
                    self.offered_from = self.media.replace(media);
                    vec![Self::http(Op::Renegotiate, Method::POST, url, body)]
                }
                _ => Vec::new(),
            },
            Input::RenegotiationAnswer(media) => match self.pending_answer.take() {
                Some(url) => {
                    let body = payload::media_answer(&self.local, &self.callbacks, &media);
                    self.media = Some(media);
                    vec![Self::http(Op::MediaAnswer, Method::POST, url, body)]
                }
                None => Vec::new(),
            },
            Input::Rebase { surl, session } => {
                self.callbacks = Callbacks::new(&surl, &session, &self.id);
                self.keep_alive().into_iter().collect()
            }
            Input::Accept(media) => match self.link("accept").map(str::to_owned) {
                Some(url) if self.phase == Phase::Notified => {
                    let body = payload::accept(&self.local, &self.callbacks, &media);
                    self.media = Some(media);
                    self.phase = Phase::Connecting;
                    vec![Self::http(Op::Accept, Method::POST, url, body)]
                }
                _ => self.end(EndReason::local(481, "NoAcceptLink")),
            },
            Input::Decline => match self.link("reject").map(str::to_owned) {
                Some(url) if self.phase == Phase::Notified => {
                    self.phase = Phase::Ending;
                    vec![Self::http(
                        Op::Reject,
                        Method::DELETE,
                        url,
                        payload::reject(),
                    )]
                }
                _ => self.end(EndReason::local(603, "Declined")),
            },
            Input::SetMuted(muted) => {
                let name = if muted { "mute" } else { "unmute" };
                self.muted = muted;
                match self.link(name).map(str::to_owned) {
                    Some(url) => vec![Self::http(
                        Op::Mute(muted),
                        Method::POST,
                        url,
                        payload::mute(&self.local, muted),
                    )],
                    None => Vec::new(),
                }
            }
            Input::StartRecording(request) => self.start_recording(request),
            Input::StopRecording => self.stop_recording(),
            Input::Timeout(Timer::Establish) => match self.phase {
                // Nobody answered here: a missed call, the caller's side times out on its own.
                Phase::Notified => self.end(EndReason::local(0, MISSED)),
                Phase::Connecting | Phase::Ringing => {
                    self.hang_up(EndReason::local(408, "Timeout"))
                }
                _ => Vec::new(),
            },
            Input::Timeout(Timer::KeepAlive) => {
                let mut commands: Vec<Command> = self.keep_alive().into_iter().collect();
                commands.push(Command::Timer(Timer::KeepAlive, KEEP_ALIVE));
                commands
            }
        }
    }

    /// Renews the callback links of the conversation.
    fn keep_alive(&self) -> Option<Command> {
        let url = self.link("notificationLinks")?.to_owned();
        Some(Self::http(
            Op::KeepAlive,
            Method::POST,
            url,
            payload::notification_links(&self.local, &self.callbacks),
        ))
    }

    /// Why we left, once the server confirmed it.
    fn left(&mut self) -> EndReason {
        self.leaving
            .take()
            .unwrap_or_else(|| EndReason::local(0, "LocalUserEnded"))
    }

    /// `callAcceptanceAcknowledgement`, once, when the service hands its link.
    fn acknowledge(&mut self, body: &Value) -> Vec<Command> {
        let link = text(body.pointer("/callAcceptance/links/acknowledgement"))
            .or_else(|| text(body.pointer("/links/acknowledgement")));
        match link {
            Some(url) if !self.acknowledged => {
                self.acknowledged = true;
                vec![Self::http(
                    Op::AcceptAck,
                    Method::POST,
                    url,
                    payload::acceptance_acknowledgement(&self.callbacks),
                )]
            }
            _ => Vec::new(),
        }
    }

    /// The final answer comes with acceptance itself, not necessarily in a separate mediaAnswer callback.
    fn accepted(&mut self, body: &Value) -> Vec<Command> {
        self.apply(body);
        if self.acknowledged {
            return Vec::new();
        }
        if matches!(self.phase, Phase::Connecting | Phase::Ringing) {
            // A hosted call is accepted by the mixer at once, while the people called still ring.
            let from_mixer = body
                .pointer("/callAcceptance/mediaContent/fromMixer")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            self.mixed = from_mixer;
            if from_mixer
                && matches!(self.kind, CallKind::Outgoing { .. })
                && !self.someone_called_answered()
            {
                self.awaiting_answer = true;
                self.phase = Phase::Ringing;
            } else {
                self.phase = Phase::Connected;
            }
        }
        let mut commands = self.acknowledge(body);
        if let Some(blob) = text(body.pointer("/callAcceptance/mediaContent/blob")) {
            self.negotiated(&blob);
            commands.push(Command::RemoteMedia(blob));
        }
        commands.extend(self.add_echo_bot_when_connected());
        commands.extend(self.remove_other_devices());
        commands.extend(self.request_videos());
        commands
    }

    /// Like `addParticipantService`, waits for the call to connect: after a 201 the command links may not work yet.
    fn add_echo_bot_when_connected(&mut self) -> Vec<Command> {
        if self.phase != Phase::Connected || self.echo_bot_added {
            return Vec::new();
        }
        let CallKind::EchoTest { bot, chat_thread } = &self.kind else {
            return Vec::new();
        };
        match self.link("addParticipant").map(str::to_owned) {
            Some(url) => {
                let body = payload::add_participant(
                    &self.local,
                    &self.callbacks,
                    std::slice::from_ref(bot),
                    chat_thread.as_deref(),
                );
                self.echo_bot_added = true;
                vec![Self::http(Op::AddParticipant, Method::POST, url, body)]
            }
            None => self.hang_up(EndReason::local(480, ECHO_BOT_UNAVAILABLE)),
        }
    }

    /// The call runs through the service's mixer, even with a single other person.
    pub fn hosted(&self) -> bool {
        self.mixed
    }

    /// Our devices other than this endpoint that are in the call.
    pub fn other_devices(&self) -> Vec<String> {
        self.roster
            .values()
            .find(|entry| entry.id.eq_ignore_ascii_case(&self.local.id))
            .map(|me| {
                me.call_endpoints
                    .iter()
                    .filter(|e| !e.eq_ignore_ascii_case(&self.local.endpoint_id))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    fn remove_other_devices(&mut self) -> Vec<Command> {
        if !self.replace_other_devices || self.phase != Phase::Connected {
            return Vec::new();
        }
        let endpoints = self.other_devices();
        let Some(url) = self.link("removeParticipant").map(str::to_owned) else {
            return Vec::new();
        };
        if endpoints.is_empty() {
            return Vec::new();
        }
        self.replace_other_devices = false;
        let body = payload::remove_endpoints(&self.local, &self.callbacks, &endpoints);
        vec![Self::http(Op::RemoveEndpoints, Method::POST, url, body)]
    }

    /// `controlInfo[].sourceId` names our line by `x-source-streamid`; `fmtParams` (or `fmtParamsList[0]`) bound it.
    fn send_limits(&self, body: &Value) -> Vec<Command> {
        let infos = body
            .pointer("/controlVideoStreaming/controlInfo")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        infos
            .iter()
            .filter_map(|info| {
                let source = info.get("sourceId").and_then(Value::as_u64)? as u32;
                let slot = self.video_slots.iter().find(|s| s.stream_id == source)?;
                let params = info
                    .pointer("/fmtParamsList/0")
                    .or_else(|| info.get("fmtParams"))
                    .and_then(Value::as_str)?;
                let get = |key: &str| {
                    params.split(';').find_map(|kv| {
                        let (k, v) = kv.split_once('=')?;
                        k.trim()
                            .eq_ignore_ascii_case(key)
                            .then(|| v.trim().to_owned())
                    })
                };
                let number = |key: &str| get(key).and_then(|v| v.parse::<f64>().ok());
                Some(Command::SendLimits(SendLimits {
                    mid: slot.mid.clone(),
                    sharing: slot.sharing,
                    max_fs: number("max-fs")? as u32,
                    max_fps: number("max-fps").map_or(30.0, |f| f / 100.0),
                    max_bitrate: number("max-br").map_or(0, |b| (b * 1200.0) as u32),
                    keyframe: get("KeyFrame").is_none_or(|k| k != "0"),
                }))
            })
            .collect()
    }

    /// Lines of the description the service just sent; earlier requests no longer hold.
    /// Restores the media of the last negotiation, and offers again while retries remain.
    fn offer_rejected(&mut self, retryable: bool) -> Vec<Command> {
        let Some(media) = self.offered_from.take() else {
            return Vec::new();
        };
        self.media = Some(media);
        self.rejected_offers += 1;
        let retry = retryable && self.rejected_offers <= RENEGOTIATION_RETRIES;
        if !retry {
            self.rejected_offers = 0;
        }
        vec![Command::LocalOfferRejected { retry }]
    }

    fn negotiated(&mut self, ngc: &str) {
        self.video_slots = crate::sdp::video_slots(ngc);
        self.video_assigned.clear();
    }

    /// Video line → participant MRI and whether it shows a shared screen.
    pub fn video_views(&self) -> Vec<(String, String, bool)> {
        self.video_slots
            .iter()
            .filter_map(|slot| {
                let (_, who) = self.video_assigned.get(&slot.mid)?;
                Some((slot.mid.clone(), who.clone(), slot.sharing))
            })
            .collect()
    }

    /// The screen being shared goes on the sharing line, cameras on the others, in roster order.
    fn request_videos(&mut self) -> Vec<Command> {
        if self.phase != Phase::Connected {
            return Vec::new();
        }
        let Some(url) = self.link("applyChannelParameters").map(str::to_owned) else {
            return Vec::new();
        };
        let others: Vec<&RosterEntry> = self
            .roster
            .values()
            .filter(|e| !e.id.eq_ignore_ascii_case(&self.local.id))
            .collect();
        let mut cameras = others
            .iter()
            .flat_map(|e| e.video_sources.first().map(|s| (*s, e.id.clone())));
        let screen = others
            .iter()
            .find_map(|e| e.sharing_sources.first().map(|s| (*s, e.id.clone())));
        let mut wanted = Vec::new();
        for slot in &self.video_slots {
            let source = if slot.sharing {
                screen.clone()
            } else {
                cameras.next()
            };
            wanted.push((slot.clone(), source));
        }
        let shown = wanted
            .iter()
            .filter(|(slot, source)| !slot.sharing && source.is_some())
            .count();
        // Another count changes every camera's size: ask them all again.
        if shown != self.cameras_shown {
            self.cameras_shown = shown;
            let sharing: Vec<String> = self
                .video_slots
                .iter()
                .filter(|s| s.sharing)
                .map(|s| s.mid.clone())
                .collect();
            self.video_assigned.retain(|mid, _| sharing.contains(mid));
        }
        let mut commands = Vec::new();
        for (slot, source) in wanted {
            match source {
                Some(source) if self.video_assigned.get(&slot.mid) != Some(&source) => {
                    self.video_sequence += 1;
                    let body = payload::source_request(&slot, source.0, shown, self.video_sequence);
                    self.video_assigned.insert(slot.mid.clone(), source);
                    commands.push(Self::http(
                        Op::SourceRequest,
                        Method::POST,
                        url.clone(),
                        body,
                    ));
                }
                Some(_) => {}
                None => {
                    self.video_assigned.remove(&slot.mid);
                }
            }
        }
        commands
    }

    /// The recorder bot, while it is in the call.
    pub fn recorder(&self) -> Option<&Recorder> {
        self.roster.values().find_map(|entry| entry.recorder.as_ref())
    }

    fn chat_thread(&self) -> Option<&str> {
        match &self.kind {
            CallKind::Meeting(meeting) => Some(&meeting.thread_id),
            CallKind::Outgoing { chat_thread, .. } | CallKind::EchoTest { chat_thread, .. } => {
                chat_thread.as_deref()
            }
            CallKind::Incoming(_) => None,
        }
    }

    fn recorder_command(&self, start: Option<&payload::recording::Request>) -> Option<Command> {
        let url = self.recorder()?.command_url.clone()?;
        Some(Self::http(
            Op::RecorderCommand(start.is_some()),
            Method::POST,
            url,
            payload::recording::command(&self.local, start),
        ))
    }

    /// `startRecording`: invite the bot, or tell it to start when it is already in the call.
    fn start_recording(&mut self, request: payload::recording::Request) -> Vec<Command> {
        if self.phase != Phase::Connected
            || matches!(self.recording, Recording::Inviting(_) | Recording::Starting)
        {
            return Vec::new();
        }
        if let Some(command) = self.recorder_command(Some(&request)) {
            self.recording = Recording::Starting;
            return vec![command];
        }
        // `addParticipantAndModality` when the call has a thread, like the web client.
        let link = self
            .chat_thread()
            .and_then(|_| self.link("addParticipantAndModality"))
            .or_else(|| self.link("addParticipant"))
            .map(str::to_owned);
        let Some(url) = link else {
            self.recording = Recording::Failed;
            return Vec::new();
        };
        let body = payload::recording::invite(
            &self.local,
            &self.callbacks,
            &self.id,
            self.chat_thread(),
            &request,
        );
        self.recording = Recording::Inviting(request);
        vec![Self::http(Op::RecorderInvite, Method::POST, url, body)]
    }

    fn stop_recording(&mut self) -> Vec<Command> {
        match self.recorder_command(None) {
            Some(command) => {
                self.recording = Recording::Stopping;
                vec![command]
            }
            None => {
                self.recording = Recording::Idle;
                Vec::new()
            }
        }
    }

    /// `onBotStateChanged`: once the invited bot is in, send `start`; settle once it reports.
    fn drive_recording(&mut self) -> Vec<Command> {
        let state = self.recorder().map(|r| r.state);
        match &self.recording {
            Recording::Inviting(request) => {
                if state.is_some_and(|s| s != RecorderState::Active) {
                    let request = request.clone();
                    if let Some(command) = self.recorder_command(Some(&request)) {
                        self.recording = Recording::Starting;
                        return vec![command];
                    }
                } else if state == Some(RecorderState::Active) {
                    self.recording = Recording::Idle;
                }
            }
            Recording::Starting if state.is_some_and(|s| s != RecorderState::Inactive) => {
                self.recording = Recording::Idle;
            }
            Recording::Stopping if state.is_none_or(|s| s == RecorderState::Inactive) => {
                self.recording = Recording::Idle;
            }
            _ => {}
        }
        Vec::new()
    }

    fn someone_called_answered(&self) -> bool {
        let CallKind::Outgoing { to, .. } = &self.kind else {
            return false;
        };
        self.roster.values().any(|entry| {
            !entry.call_endpoints.is_empty()
                && to.iter().any(|t| t.id.eq_ignore_ascii_case(&entry.id))
        })
    }

    fn promote_when_answered(&mut self) {
        if self.awaiting_answer && self.phase == Phase::Ringing && self.someone_called_answered() {
            self.awaiting_answer = false;
            self.phase = Phase::Connected;
        }
    }

    fn hang_up(&mut self, reason: EndReason) -> Vec<Command> {
        // A second hang-up while leaving stops waiting for the server.
        if self.phase == Phase::Ending {
            return self.end(reason);
        }
        let unanswered = matches!(self.kind, CallKind::Outgoing { .. })
            && matches!(self.phase, Phase::Connecting | Phase::Ringing);
        if !unanswered {
            return self.leave(reason);
        }
        // Leaving alone keeps the others ringing in the hosted conversation: end it for everyone.
        let reason = if reason.code == 0 {
            EndReason::local(487, LOCAL_USER_INITIATED)
        } else {
            reason
        };
        match self.controller.clone() {
            Some(url) => {
                self.phase = Phase::Ending;
                let body = payload::end_for_everyone(
                    &self.local,
                    payload::call_end(reason.code, reason.sub_code, &reason.phrase),
                );
                self.leaving = Some(reason);
                vec![Self::http(Op::EndForAll, Method::DELETE, url, body)]
            }
            None => self.leave(reason),
        }
    }

    fn leave(&mut self, reason: EndReason) -> Vec<Command> {
        match self.link("leave").map(str::to_owned) {
            Some(url) => {
                self.phase = Phase::Ending;
                let mut body = payload::leave(&self.local);
                body["callTransactionEnd"] =
                    payload::call_end(reason.code, reason.sub_code, &reason.phrase);
                self.leaving = Some(reason);
                vec![Self::http(Op::Leave, Method::POST, url, body)]
            }
            // Nothing established server-side yet: just stop here.
            None => self.end(reason),
        }
    }

    fn response(&mut self, op: Op, result: Result<Value, u16>) -> Vec<Command> {
        match op {
            // Like the web client, which cancels the pending join when redirected.
            Op::Join | Op::Start if self.redirects > 0 => return Vec::new(),
            Op::Redirected(n) if n != self.redirects => return Vec::new(),
            _ => {}
        }
        match (op, result) {
            (Op::Join | Op::Start | Op::Redirected(_), Ok(body)) => {
                self.apply(&body);
                if self.subscribed() && self.phase == Phase::Connecting {
                    self.phase = Phase::Connected;
                }
                let mut commands = vec![Command::Timer(Timer::KeepAlive, KEEP_ALIVE)];
                commands.extend(self.add_echo_bot_when_connected());
                commands
            }
            (Op::AddParticipant, Ok(body)) => {
                self.apply(&body);
                Vec::new()
            }
            (Op::AddParticipant, Err(_)) => {
                self.hang_up(EndReason::local(480, ECHO_BOT_UNAVAILABLE))
            }
            (Op::Join | Op::Start | Op::Redirected(_), Err(status)) => self.end(EndReason {
                code: status,
                sub_code: 0,
                phrase: "ConversationServiceError".into(),
                local: true,
            }),
            (Op::Attach, Ok(mut body)) => {
                // `participants` is `{from, to}` of the invitation, not a roster: read as one, it adds both people twice.
                if let Some(body) = body.as_object_mut() {
                    body.remove("participants");
                }
                self.apply(&body);
                // The answer to the join action carries the conversation links.
                if let Some(join) = body
                    .get("additionalActionResponses")
                    .and_then(Value::as_array)
                    .and_then(|r| r.first())
                    .and_then(|r| r.get("output").or_else(|| r.get("response")))
                {
                    self.apply(join);
                }
                // `processAttachResponse`: accept on `acceptance`, decline with a DELETE on `callLeg`.
                for (name, link) in [("accept", "acceptance"), ("reject", "callLeg")] {
                    if let Some(url) = text(body.pointer(&format!("/callInvitation/links/{link}"))) {
                        self.links.insert(name.into(), url);
                    }
                }
                let offer = [
                    body.get("mediaContent"),
                    body.pointer("/attach/mediaContent"),
                    body.pointer("/callInvitation/mediaContent"),
                ]
                .into_iter()
                .flatten()
                .find_map(|m| text(m.get("blob")));
                let mut commands: Vec<Command> =
                    offer.map(Command::RemoteMedia).into_iter().collect();
                if let Some(url) = self.link("progress").map(str::to_owned) {
                    commands.push(Self::http(
                        Op::Progress,
                        Method::POST,
                        url,
                        payload::ringing(&self.local),
                    ));
                }
                commands
            }
            (Op::Attach, Err(status)) => self.end(EndReason::local(status, "AttachFailed")),
            (Op::Accept, Ok(body)) => {
                self.apply(&body);
                if body.get("callAcceptanceAcknowledgement").is_some()
                    && self.phase == Phase::Connecting
                {
                    self.phase = Phase::Connected;
                }
                Vec::new()
            }
            (Op::Accept, Err(status)) => self.end(EndReason::local(status, "AcceptFailed")),
            (Op::Renegotiate | Op::MediaAnswer, Ok(body)) => {
                self.apply(&body);
                Vec::new()
            }
            // Like the web client, a 404 means the renegotiation link is gone: not worth retrying.
            (Op::Renegotiate, Err(status)) => self.offer_rejected(status != 404),
            (Op::MediaAnswer, Err(_)) => Vec::new(),
            (Op::RemoveEndpoints, _) => Vec::new(),
            (Op::SourceRequest, Ok(_)) => Vec::new(),
            // Asked again on the next roster or negotiation change.
            (Op::SourceRequest, Err(_)) => {
                self.video_assigned.clear();
                Vec::new()
            }
            (Op::AcceptAck | Op::MediaAck | Op::Progress, Ok(body)) => {
                self.apply(&body);
                Vec::new()
            }
            (Op::AcceptAck | Op::MediaAck | Op::Progress, Err(_)) => Vec::new(),
            // Not allowed to end it for everyone: at least leave.
            (Op::EndForAll, Err(_)) => {
                let reason = self
                    .leaving
                    .take()
                    .unwrap_or_else(|| EndReason::local(487, LOCAL_USER_INITIATED));
                self.leave(reason)
            }
            (Op::Leave | Op::Reject | Op::EndForAll, _) => {
                let reason = match op {
                    Op::Reject => EndReason::local(603, "Declined"),
                    _ => self.left(),
                };
                // Whatever the server answered, the call is over on this side.
                self.end(reason)
            }
            (Op::Mute(muted), Err(_)) => {
                self.muted = !muted;
                Vec::new()
            }
            (Op::Mute(_) | Op::KeepAlive, Ok(body)) => {
                self.apply(&body);
                Vec::new()
            }
            (Op::KeepAlive, Err(_)) => Vec::new(),
            (Op::RecorderInvite, Ok(body)) => {
                self.apply(&body);
                self.drive_recording()
            }
            (Op::RecorderInvite | Op::RecorderCommand(_), Err(_)) => {
                self.recording = Recording::Failed;
                Vec::new()
            }
            (Op::RecorderCommand(_), Ok(_)) => self.drive_recording(),
        }
    }

    fn callback(&mut self, scope: Scope, name: &str, body: Value) -> Vec<Command> {
        match (scope, name) {
            (Scope::Conversation, "conversationEnd") | (Scope::Call, "end") => {
                let reason = EndReason::from_body(&body);
                // 409 redirect (sub-code 5704): join again at the given conversation URL.
                let redirect = text(
                    body.get("conversationUrl")
                        .and_then(|u| u.get("Location").or_else(|| u.get("location"))),
                );
                if reason.sub_code == 5704
                    && let Some(url) = redirect
                    && !matches!(self.kind, CallKind::Incoming(_))
                {
                    self.join_url = url;
                    self.redirects += 1;
                    self.controller = None;
                    self.links.clear();
                    self.sequence = None;
                    self.phase = Phase::Connecting;
                    return vec![self.join_command()];
                }
                if self.phase == Phase::Ending {
                    let reason = self.left();
                    return self.end(reason);
                }
                if self.phase == Phase::Notified {
                    return self.end(EndReason {
                        phrase: MISSED.into(),
                        ..reason
                    });
                }
                self.end(reason)
            }
            (Scope::Conversation, "addParticipantFailure")
                if matches!(self.recording, Recording::Inviting(_)) =>
            {
                self.recording = Recording::Failed;
                Vec::new()
            }
            (Scope::Conversation, "addParticipantFailure")
                if matches!(self.kind, CallKind::EchoTest { .. }) =>
            {
                self.hang_up(EndReason::local(480, ECHO_BOT_UNAVAILABLE))
            }
            (Scope::Conversation, "rosterUpdate") => {
                // Not the conversation's `sequenceNumber`: roster updates are numbered apart.
                let roster = body
                    .get("roster")
                    .or_else(|| body.get("rosterUpdate"))
                    .unwrap_or(&body);
                self.apply_roster(roster);
                // A test call with nobody rung is up once the bot is in.
                if let CallKind::EchoTest { bot, .. } = &self.kind
                    && self.phase == Phase::Connecting
                    && self
                        .roster
                        .keys()
                        .any(|id| id.eq_ignore_ascii_case(&bot.id))
                {
                    self.phase = Phase::Connected;
                }
                self.promote_when_answered();
                let mut commands = self.remove_other_devices();
                commands.extend(self.request_videos());
                commands.extend(self.drive_recording());
                commands
            }
            (Scope::Conversation, _) => {
                self.apply(&body);
                Vec::new()
            }
            (Scope::Call, "progress") => {
                self.apply(&body);
                if self.phase == Phase::Connecting {
                    self.phase = Phase::Ringing;
                }
                Vec::new()
            }
            (Scope::Call, "mediaAnswer") => {
                self.apply(&body);
                let mut commands = Vec::new();
                if let Some(url) = text(body.pointer("/mediaAnswer/links/mediaAcknowledgement")) {
                    commands.push(Self::http(Op::MediaAck, Method::POST, url, Value::Null));
                }
                let blob = body
                    .get("mediaAnswer")
                    .and_then(|m| m.get("mediaContent"))
                    .or_else(|| body.get("mediaContent"))
                    .and_then(|m| text(m.get("blob")));
                let provisional = matches!(self.phase, Phase::Connecting | Phase::Ringing)
                    && body
                        .pointer("/mediaAnswer/links/mediaAcknowledgement")
                        .is_none();
                commands.extend(blob.map(|blob| {
                    if provisional {
                        Command::RemoteProvisionalMedia(blob)
                    } else {
                        self.offered_from = None;
                        self.rejected_offers = 0;
                        self.negotiated(&blob);
                        Command::RemoteMedia(blob)
                    }
                }));
                commands.extend(self.request_videos());
                commands
            }
            (Scope::Call, "mediaRenegotiation" | "newMediaOffer") => {
                self.apply(&body);
                let negotiation = body.get("mediaNegotiation").unwrap_or(&body);
                let offer = negotiation
                    .pointer("/mediaContent/blob")
                    .and_then(Value::as_str);
                let answer = negotiation
                    .pointer("/links/mediaAnswer")
                    .and_then(Value::as_str);
                match (offer, answer) {
                    (Some(offer), Some(answer)) => {
                        self.negotiated(offer);
                        self.pending_answer = Some(answer.to_owned());
                        vec![Command::RemoteOffer(offer.to_owned())]
                    }
                    _ => Vec::new(),
                }
            }
            (Scope::Call, "acknowledgement") => {
                self.apply(&body);
                if self.phase == Phase::Connecting {
                    self.phase = Phase::Connected;
                }
                Vec::new()
            }
            (Scope::Call, "acceptance") => self.accepted(&body),
            (Scope::Call, "mediaRejection") => {
                self.apply(&body);
                self.offer_rejected(true)
            }
            // A renegotiation is complete: its lines can carry video now.
            (Scope::Call, "controlVideoStreaming") => {
                self.apply(&body);
                self.send_limits(&body)
            }
            (Scope::Call, "mediaAcknowledgement") => {
                self.apply(&body);
                self.request_videos()
            }
            (Scope::Call, _) => {
                self.apply(&body);
                Vec::new()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn acceptance_delivers_the_final_media_answer_and_acknowledges_once() {
        let mut session = meeting(Some(Media {
            video: true,
            screen: false,
            content: payload::media_content("offer"),
        }));
        session.handle(Input::Begin);
        joined(&mut session);
        let early = callback(
            &mut session,
            Scope::Call,
            "mediaAnswer",
            json!({"mediaAnswer": {
                "mediaContent": {"blob": "early-answer"}
            }}),
        );
        assert_eq!(
            early,
            [Command::RemoteProvisionalMedia("early-answer".into())]
        );
        assert_eq!(session.phase, Phase::Connecting);
        let body = json!({"callAcceptance": {
            "mediaContent": {"contentType": "application/sdp-ngc-1.0", "blob": "final-answer"},
            "links": {"acknowledgement": "https://conv.example/ack",
                      "mediaRenegotiation": "https://conv.example/reneg"}
        }});
        let commands = callback(&mut session, Scope::Call, "acceptance", body.clone());
        assert_eq!(session.phase, Phase::Connected);
        assert_eq!(
            session.link("mediaRenegotiation"),
            Some("https://conv.example/reneg")
        );
        assert_eq!(commands.len(), 2);
        assert!(
            matches!(&commands[0], Command::Http { op: Op::AcceptAck, method, url, body }
            if method == Method::POST && url == "https://conv.example/ack"
            && body["callAcceptanceAcknowledgement"]["links"]["mediaRenegotiation"].is_string())
        );
        assert_eq!(commands[1], Command::RemoteMedia("final-answer".into()));
        assert!(callback(&mut session, Scope::Call, "acceptance", body).is_empty());
    }

    #[test]
    fn renegotiation_answers_are_acknowledged() {
        let mut session = meeting(Some(Media {
            video: false,
            screen: false,
            content: payload::media_content("offer"),
        }));
        let commands = callback(
            &mut session,
            Scope::Call,
            "mediaAnswer",
            json!({"mediaAnswer": {
                "mediaContent": {"blob": "answer"},
                "links": {"mediaAcknowledgement": "https://conv.example/media-ack"}
            }}),
        );
        assert!(
            matches!(&commands[0], Command::Http { op: Op::MediaAck, method, url, body }
            if method == Method::POST && url == "https://conv.example/media-ack" && body.is_null())
        );
        assert_eq!(commands[1], Command::RemoteMedia("answer".into()));
    }

    fn local() -> Local {
        Local {
            id: "8:orgid:me".into(),
            display_name: "Moi".into(),
            endpoint_id: "ep".into(),
            participant_id: "p".into(),
            language_id: "fr-fr".into(),
        }
    }
    fn meeting(media: Option<Media>) -> CallSession {
        CallSession::new(
            "call-1".into(),
            local(),
            Callbacks::new("https://t.example/s", "sess", "call-1"),
            CallKind::Meeting(MeetingCoordinates {
                thread_id: "19:meeting_x@thread.v2".into(),
                message_id: "0".into(),
                tenant_id: Some("t".into()),
                organizer_id: Some("o".into()),
            }),
            media,
            "https://conv.example/epconv/".into(),
        )
    }
    fn joined(session: &mut CallSession) {
        session.handle(Input::Begin);
        session.handle(Input::Response(
            Op::Join,
            Ok(json!({"conversationController": "https://conv.example/c1", "sequenceNumber": 1,
                      "links": {"leave": "https://conv.example/c1/leave", "mute": "https://conv.example/c1/mute",
                                "notificationLinks": "https://conv.example/c1/links"}})),
        ));
    }
    fn callback(session: &mut CallSession, scope: Scope, name: &str, body: Value) -> Vec<Command> {
        session.handle(Input::Callback {
            scope,
            name: name.into(),
            body,
        })
    }

    #[test]
    fn roster_only_join_connects_on_the_response_and_tracks_the_roster() {
        let mut session = meeting(None);
        let commands = session.handle(Input::Begin);
        assert_eq!(
            commands.len(),
            1,
            "no establish timer for a roster-only join"
        );
        let Command::Http {
            op,
            method,
            url,
            body,
        } = &commands[0]
        else {
            panic!()
        };
        assert_eq!(
            (*op, method, url.as_str()),
            (Op::Join, &Method::POST, "https://conv.example/epconv/")
        );
        assert_eq!(body["subscribe"], true);
        joined(&mut session);
        assert_eq!(session.phase, Phase::Connected);
        callback(
            &mut session,
            Scope::Conversation,
            "rosterUpdate",
            json!({"sequenceNumber": 2,
            "participants": {"8:orgid:a": {"version": 2, "displayName": "Alice", "role": "organizer"},
                             "8:orgid:b": {"displayName": "Bob"}}}),
        );
        assert_eq!(session.roster.len(), 2);
        assert_eq!(
            session.roster["8:orgid:a"].role.as_deref(),
            Some("organizer")
        );
        // Stale delta: ignored.
        callback(
            &mut session,
            Scope::Conversation,
            "rosterUpdate",
            json!({"sequenceNumber": 4, "participants": {"8:orgid:a": {"version": 1, "state": "inactive"}}}),
        );
        assert_eq!(session.roster.len(), 2);
        callback(
            &mut session,
            Scope::Conversation,
            "rosterUpdate",
            json!({"sequenceNumber": 3, "participants": {"8:orgid:b": null}}),
        );
        assert_eq!(session.roster.keys().collect::<Vec<_>>(), ["8:orgid:a"]);
    }

    #[test]
    fn lobby_admission_media_answer_and_leave() {
        let mut session = meeting(Some(Media {
            video: false,
            screen: false,
            content: payload::media_content("offer"),
        }));
        let commands = session.handle(Input::Begin);
        assert!(commands.contains(&Command::Timer(Timer::Establish, ESTABLISH_TIMEOUT)));
        joined(&mut session);
        assert_eq!(session.phase, Phase::Connecting);
        callback(
            &mut session,
            Scope::Conversation,
            "localParticipantUpdate",
            json!({"selfParticipant": {"state": "InLobby"}}),
        );
        assert_eq!(session.phase, Phase::Lobby);
        assert!(
            session.handle(Input::Timeout(Timer::Establish)).is_empty(),
            "waiting in the lobby is fine"
        );
        callback(
            &mut session,
            Scope::Conversation,
            "localParticipantUpdate",
            json!({"selfParticipant": {"state": "Connected"}}),
        );
        assert_eq!(session.phase, Phase::Connected);
        let commands = callback(
            &mut session,
            Scope::Call,
            "mediaAnswer",
            json!({"mediaAnswer": {"mediaContent": {"contentType": "application/sdp-ngc-1.0", "blob": "answer"}}}),
        );
        assert_eq!(commands, [Command::RemoteMedia("answer".into())]);

        let commands = session.handle(Input::SetMuted(true));
        assert!(
            matches!(&commands[0], Command::Http { op: Op::Mute(true), url, .. } if url == "https://conv.example/c1/mute")
        );
        assert!(
            session.handle(Input::SetMuted(false)).is_empty(),
            "no unmute link"
        );

        let commands = session.handle(Input::Hangup);
        let Command::Http {
            op: Op::Leave,
            url,
            body,
            ..
        } = &commands[0]
        else {
            panic!()
        };
        assert_eq!(url, "https://conv.example/c1/leave");
        assert_eq!(body["participants"]["from"]["id"], "8:orgid:me");
        assert_eq!(session.phase, Phase::Ending);
        session.handle(Input::Response(Op::Leave, Err(500)));
        assert!(matches!(&session.phase, Phase::Ended(r) if r.local && r.code == 0));
    }

    #[test]
    fn conflict_redirects_the_join_and_remote_ends_are_reported() {
        let mut session = meeting(None);
        joined(&mut session);
        let commands = callback(
            &mut session,
            Scope::Conversation,
            "conversationEnd",
            json!({"code": 409, "subCode": 5704, "conversationUrl": {"Location": "https://conv2.example/c9"}}),
        );
        let Command::Http {
            op: Op::Redirected(1),
            url,
            ..
        } = &commands[0]
        else {
            panic!()
        };
        assert_eq!(url, "https://conv2.example/c9");
        assert_eq!(session.phase, Phase::Connecting);
        // The first join is answered late, with an error: the redirected join goes on.
        session.handle(Input::Response(Op::Join, Err(426)));
        assert_eq!(session.phase, Phase::Connecting);
        session.handle(Input::Response(
            Op::Redirected(1),
            Ok(json!({"links": {"leave": "https://conv2.example/c9/leave"}})),
        ));
        assert_eq!(
            session.link("leave"),
            Some("https://conv2.example/c9/leave")
        );
        callback(
            &mut session,
            Scope::Conversation,
            "conversationEnd",
            json!({"conversationEnd": {"code": 0, "subCode": 0, "phrase": "MeetingEnded"}}),
        );
        let Phase::Ended(reason) = &session.phase else {
            panic!()
        };
        assert!(!reason.local);
        assert_eq!(reason.message(), "L’appel est terminé.");
        assert!(
            session.handle(Input::Hangup).is_empty(),
            "nothing after the end"
        );
    }

    #[test]
    fn calling_a_chat_rings_until_the_callee_joins_and_hanging_up_cancels_for_everyone() {
        let start = || {
            let mut session = CallSession::new(
                "c".into(),
                local(),
                Callbacks::new("https://t.example/s", "sess", "c"),
                CallKind::Outgoing {
                    to: vec![Target {
                        id: "8:orgid:bob".into(),
                        display_name: None,
                    }],
                    chat_thread: Some("19:chat@unq.gbl.spaces".into()),
                },
                Some(Media {
                    video: false,
                    screen: false,
                    content: payload::media_content("offer"),
                }),
                "https://conv.example/epconv/".into(),
            );
            session.handle(Input::Begin);
            session.handle(Input::Response(
                Op::Start,
                Ok(json!({"conversationController": "https://conv.example/c1",
                          "links": {"leave": "https://conv.example/c1/leave"}})),
            ));
            // The hosted conversation's mixer takes our leg at once.
            let commands = callback(
                &mut session,
                Scope::Call,
                "acceptance",
                json!({"callAcceptance": {"mediaContent": {"blob": "mixer", "fromMixer": true}}}),
            );
            assert_eq!(commands, [Command::RemoteMedia("mixer".into())]);
            assert_eq!(session.phase, Phase::Ringing);
            assert!(
                session.hosted(),
                "the mixer hosts it: conference SDP rules apply"
            );
            session
        };
        let roster = |endpoints: Value| {
            json!({"sequenceNumber": 2, "participants": {
                "8:orgid:me": {"state": "active", "details": {"id": "8:orgid:me"},
                               "endpoints": {"ep": {"call": {}}}},
                "8:orgid:bob": {"state": "active", "details": {"id": "8:orgid:bob", "displayName": "Bob"},
                                "endpoints": endpoints},
            }})
        };

        let mut answered = start();
        callback(
            &mut answered,
            Scope::Conversation,
            "rosterUpdate",
            roster(json!({"bob-ep": {}})),
        );
        assert_eq!(answered.phase, Phase::Ringing, "only ourselves in the call");
        callback(
            &mut answered,
            Scope::Conversation,
            "rosterUpdate",
            roster(json!({"bob-ep": {"call": {"mediaStreams": []}}})),
        );
        assert_eq!(answered.phase, Phase::Connected);
        let commands = answered.handle(Input::Hangup);
        assert!(matches!(
            &commands[..],
            [Command::Http { op: Op::Leave, .. }]
        ));

        let mut cancelled = start();
        let commands = cancelled.handle(Input::Hangup);
        let [
            Command::Http {
                op: Op::EndForAll,
                method,
                url,
                body,
            },
        ] = &commands[..]
        else {
            panic!("{commands:?}");
        };
        assert_eq!(
            (method, url.as_str()),
            (&Method::DELETE, "https://conv.example/c1")
        );
        assert_eq!(body["callTransactionEnd"]["code"], 487);
        assert_eq!(
            body["conversationTransactionEnd"]["phrase"],
            "ConversationEndForAllInitiated"
        );
        cancelled.handle(Input::Response(Op::EndForAll, Ok(Value::Null)));
        assert!(matches!(&cancelled.phase, Phase::Ended(r) if r.code == 487));

        // Not allowed to end it for everyone: leave with the same reason.
        let mut refused = start();
        refused.handle(Input::Hangup);
        let commands = refused.handle(Input::Response(Op::EndForAll, Err(403)));
        assert!(
            matches!(&commands[..], [Command::Http { op: Op::Leave, body, .. }] if body["callTransactionEnd"]["code"] == 487)
        );
    }

    #[test]
    fn moving_the_meeting_here_removes_our_other_devices_once_in() {
        let mut session = meeting(Some(Media {
            video: false,
            screen: false,
            content: payload::media_content("offer"),
        }));
        session.replace_other_devices = true;
        session.handle(Input::Begin);
        session.handle(Input::Response(
            Op::Join,
            Ok(json!({"links": {"removeParticipant": "https://conv.example/c1/remove"}})),
        ));
        let roster = json!({"sequenceNumber": 2, "participants": {"8:orgid:me": {
            "state": "active", "details": {"id": "8:orgid:me"},
            "endpoints": {"ep": {"call": {}}, "phone": {"call": {}}, "watch": {}},
        }}});
        let commands = callback(
            &mut session,
            Scope::Conversation,
            "rosterUpdate",
            roster.clone(),
        );
        assert!(commands.is_empty(), "not in the call yet");
        assert_eq!(session.other_devices(), ["phone"]);

        let commands = callback(&mut session, Scope::Call, "acceptance", json!({}));
        let [
            Command::Http {
                op: Op::RemoveEndpoints,
                url,
                body,
                ..
            },
        ] = &commands[..]
        else {
            panic!("{commands:?}");
        };
        assert_eq!(url, "https://conv.example/c1/remove");
        assert_eq!(
            body["participants"]["toEndpoints"],
            json!([{"id": "8:orgid:me", "endpointId": "phone"}])
        );
        assert!(
            callback(&mut session, Scope::Conversation, "rosterUpdate", roster).is_empty(),
            "only once"
        );
    }

    #[test]
    fn roster_entries_are_people_by_details_id_not_by_key() {
        let mut session = meeting(None);
        joined(&mut session);
        callback(
            &mut session,
            Scope::Conversation,
            "rosterUpdate",
            json!({"type": "MultiPartyEndpoint", "sequenceNumber": 1, "participants": {
                "leg-1": {"details": {"id": "8:orgid:bob", "displayName": "Bob"}, "endpoints": {"e1": {"call": {}}}},
                "leg-2": {"details": {"id": "8:orgid:bob", "displayName": "Bob"}, "endpoints": {"e2": {"call": {}}}},
                "leg-3": {"details": {"id": "8:orgid:carol"}, "endpoints": {"e3": {"endpointMetadata": {}}}},
            }}),
        );
        assert_eq!(session.roster.keys().collect::<Vec<_>>(), ["8:orgid:bob"]);
    }

    #[test]
    fn roster_updates_are_ordered_per_participant_not_by_conversation_updates() {
        let mut session = meeting(None);
        joined(&mut session);
        callback(
            &mut session,
            Scope::Conversation,
            "conversationUpdate",
            json!({"sequenceNumber": 50}),
        );
        let bob = |version: i64, state: &str| {
            json!({"type": "Delta", "sequenceNumber": 3, "participants": {"8:orgid:bob": {
                "version": version, "state": state,
                "details": {"id": "8:orgid:bob", "displayName": "Bob"},
                "endpoints": {"e1": {"call": {}}},
            }}})
        };
        callback(
            &mut session,
            Scope::Conversation,
            "rosterUpdate",
            bob(2, "active"),
        );
        assert_eq!(
            session.roster["8:orgid:bob"].display_name.as_deref(),
            Some("Bob")
        );
        callback(
            &mut session,
            Scope::Conversation,
            "rosterUpdate",
            json!({"type": "Delta", "participants": {"8:orgid:carol": {
                "version": 1, "state": "active", "details": {"id": "8:orgid:carol"},
                "endpoints": {"e2": {
                    "call": {"mediaStreams": [
                        {"type": "audio", "sourceId": 42, "direction": "sendrecv"},
                        {"type": "video", "sourceId": 43},
                    ]},
                    "endpointState": {"state": {"isMuted": true}},
                }},
            }}}),
        );
        let carol = &session.roster["8:orgid:carol"];
        assert_eq!(carol.audio_sources, [42]);
        assert!(carol.muted && !session.roster["8:orgid:bob"].muted);
        callback(
            &mut session,
            Scope::Conversation,
            "rosterUpdate",
            bob(1, "inactive"),
        );
        assert!(
            session.roster.contains_key("8:orgid:bob"),
            "older version ignored"
        );
        callback(
            &mut session,
            Scope::Conversation,
            "rosterUpdate",
            bob(3, "inactive"),
        );
        assert!(!session.roster.contains_key("8:orgid:bob"));
    }

    #[test]
    fn cameras_and_the_shared_screen_are_asked_on_the_negotiated_video_lines() {
        let mut session = meeting(Some(Media {
            video: false,
            screen: false,
            content: payload::media_content("offer"),
        }));
        joined(&mut session);
        let answer = "v=0\r\ns=-\r\nt=0 0\r\nm=audio 1 RTP/SAVP 111\r\na=mid:0\r\n\
            m=video 1 RTP/SAVP 96\r\na=mid:1\r\na=x-source-streamid:11\r\na=label:main-video\r\n\
            m=video 1 RTP/SAVP 96\r\na=mid:2\r\na=x-source-streamid:12\r\na=label:applicationsharing-video\r\n";
        callback(
            &mut session,
            Scope::Call,
            "acceptance",
            json!({"callAcceptance": {
                "links": {"applyChannelParameters": "https://conv.example/acp"},
                "mediaContent": {"blob": answer},
            }}),
        );
        assert_eq!(session.phase, Phase::Connected);
        let roster = json!({"participants": {"8:orgid:bob": {
            "version": 1, "state": "active", "details": {"id": "8:orgid:bob"},
            "endpoints": {"b": {"call": {"mediaStreams": [
                {"type": "audio", "sourceId": 1},
                {"type": "video", "sourceId": 7, "direction": "sendrecv"},
                {"type": "applicationsharing-video", "sourceId": 9, "direction": "sendonly"},
            ]}}},
        }}});
        let commands = callback(&mut session, Scope::Conversation, "rosterUpdate", roster);
        let requests: Vec<Value> = commands
            .iter()
            .filter_map(|c| match c {
                Command::Http {
                    op: Op::SourceRequest,
                    url,
                    body,
                    ..
                } => {
                    assert_eq!(url, "https://conv.example/acp");
                    let p = &body["applyChannelParameters"]["multiChannelParameter"];
                    let control: Value =
                        serde_json::from_str(p["mediaParameter"].as_str().unwrap()).unwrap();
                    Some(json!([
                        p["mids"][0],
                        control["controlVideoStreaming"]["controlInfo"]["sourceId"],
                        control["controlVideoStreaming"]["controlInfo"]["streamMsid"]
                    ]))
                }
                _ => None,
            })
            .collect();
        assert_eq!(requests, [json!(["1", 7, 11]), json!(["2", 9, 12])]);
        assert_eq!(
            session.video_views(),
            [
                ("1".into(), "8:orgid:bob".into(), false),
                ("2".into(), "8:orgid:bob".into(), true)
            ]
        );
        assert!(
            callback(
                &mut session,
                Scope::Conversation,
                "conversationUpdate",
                json!({})
            )
            .is_empty()
        );
    }

    #[test]
    fn every_camera_gets_its_own_line_and_a_smaller_size_when_many() {
        let mut session = meeting(Some(Media {
            video: false,
            screen: false,
            content: payload::media_content("offer"),
        }));
        joined(&mut session);
        let lines: String = (1..=4)
            .map(|i| {
                format!(
                    "m=video 1 RTP/SAVP 96\r\na=mid:{i}\r\na=x-source-streamid:{}\r\n",
                    10 + i
                )
            })
            .collect();
        let answer = format!("v=0\r\ns=-\r\nt=0 0\r\nm=audio 1 RTP/SAVP 111\r\na=mid:0\r\n{lines}");
        callback(
            &mut session,
            Scope::Call,
            "acceptance",
            json!({"callAcceptance": {
                "links": {"applyChannelParameters": "https://conv.example/acp"},
                "mediaContent": {"blob": answer},
            }}),
        );
        let person = |n: u32| {
            json!({"version": 1, "state": "active", "details": {"id": format!("8:orgid:p{n}")},
                "endpoints": {"e": {"call": {"mediaStreams": [{"type": "video", "sourceId": 100 + n}]}}}})
        };
        let commands = callback(
            &mut session,
            Scope::Conversation,
            "rosterUpdate",
            json!({"participants": {"8:orgid:p1": person(1), "8:orgid:p2": person(2), "8:orgid:p3": person(3)}}),
        );
        let sizes: Vec<(Value, Value)> = commands
            .iter()
            .filter_map(|c| match c {
                Command::Http {
                    op: Op::SourceRequest,
                    body,
                    ..
                } => {
                    let p = &body["applyChannelParameters"]["multiChannelParameter"];
                    let control: Value =
                        serde_json::from_str(p["mediaParameter"].as_str().unwrap()).unwrap();
                    let info = &control["controlVideoStreaming"]["controlInfo"];
                    Some((
                        info["sourceId"].clone(),
                        info["fmtParams"][0]["max-fs"].clone(),
                    ))
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            sizes,
            [
                (json!(101), json!(2040)),
                (json!(102), json!(2040)),
                (json!(103), json!(2040))
            ]
        );
        assert_eq!(session.video_views().len(), 3);
    }

    #[test]
    fn the_mixer_bounds_our_sending_lines_through_control_video_streaming() {
        let mut session = meeting(Some(Media {
            video: true,
            screen: false,
            content: payload::media_content("offer"),
        }));
        joined(&mut session);
        let answer = "v=0\r\ns=-\r\nt=0 0\r\nm=audio 1 RTP/SAVP 111\r\na=mid:0\r\n\
            m=video 1 RTP/SAVP 96\r\na=mid:1\r\na=x-source-streamid:202\r\na=label:main-video\r\n\
            m=video 1 RTP/SAVP 96\r\na=mid:2\r\na=x-source-streamid:203\r\na=label:applicationsharing-video\r\n";
        callback(
            &mut session,
            Scope::Call,
            "acceptance",
            json!({"callAcceptance": {"mediaContent": {"blob": answer}}}),
        );
        let commands = callback(
            &mut session,
            Scope::Call,
            "controlVideoStreaming",
            json!({"controlVideoStreaming": {"controlInfo": [
                {"control": 0, "sourceId": 203,
                 "fmtParams": "max-mbps=520000;packetization-mode=1;max-fps=3000;max-fs=32400;max-br=2935"},
                {"control": 0, "sourceId": 202, "fmtParams": "ignored",
                 "fmtParamsList": ["KeyFrame=1;max-br=123;max-fps=1500.000000;max-fs=240;ssrc=350548238"]},
                {"control": 0, "sourceId": 999, "fmtParams": "max-fs=1"},
            ]}}),
        );
        assert_eq!(
            commands,
            [
                Command::SendLimits(SendLimits {
                    mid: "2".into(),
                    sharing: true,
                    max_fs: 32400,
                    max_fps: 30.0,
                    max_bitrate: 3_522_000,
                    keyframe: true,
                }),
                Command::SendLimits(SendLimits {
                    mid: "1".into(),
                    sharing: false,
                    max_fs: 240,
                    max_fps: 15.0,
                    max_bitrate: 147_600,
                    keyframe: true,
                }),
            ]
        );
    }

    #[test]
    fn outgoing_calls_ring_connect_or_time_out() {
        let mut session = CallSession::new(
            "c".into(),
            local(),
            Callbacks::new("https://t.example/s", "sess", "c"),
            CallKind::Outgoing {
                to: vec![Target {
                    id: "8:orgid:bob".into(),
                    display_name: None,
                }],
                chat_thread: None,
            },
            Some(Media {
                video: false,
                screen: false,
                content: payload::media_content("offer"),
            }),
            "https://conv.example/epconv/".into(),
        );
        let commands = session.handle(Input::Begin);
        assert!(matches!(&commands[0], Command::Http { op: Op::Start, .. }));
        session.handle(Input::Response(
            Op::Start,
            Ok(json!({"links": {"leave": "https://conv.example/l"}})),
        ));
        callback(&mut session, Scope::Call, "progress", json!({}));
        assert_eq!(session.phase, Phase::Ringing);
        let commands = session.handle(Input::Timeout(Timer::Establish));
        assert!(
            matches!(&commands[0], Command::Http { op: Op::Leave, body, .. } if body["callTransactionEnd"]["code"] == 408)
        );
        session.handle(Input::Response(Op::Leave, Ok(Value::Null)));
        assert!(session.ended());

        let mut failed = meeting(None);
        failed.handle(Input::Begin);
        failed.handle(Input::Response(Op::Join, Err(403)));
        assert!(matches!(&failed.phase, Phase::Ended(r) if r.code == 403));
    }

    fn incoming() -> CallSession {
        CallSession::new(
            "c".into(),
            local(),
            Callbacks::new("https://t.example/s", "sess", "c"),
            CallKind::Incoming(IncomingCall {
                event_id: 107,
                caller_id: "8:orgid:bob".into(),
                caller_name: Some("Bob".into()),
                attach: "https://conv.example/attach".into(),
                conversation_controller: None,
                thread_id: None,
                multiparty: false,
                call_id: None,
            }),
            None,
            String::new(),
        )
    }

    #[test]
    fn accepting_waits_for_the_callers_acknowledgement() {
        let mut session = incoming();
        session.handle(Input::Begin);
        let commands = session.handle(Input::Response(Op::Attach, Ok(json!({
            "callInvitation": {"mediaContent": {"blob": "offer"},
                "links": {"progress": "https://conv.example/progress", "acceptance": "https://conv.example/accept"}},
            "participants": {"from": {"id": "8:orgid:caller", "displayName": "Caller"}, "to": {"id": "8:orgid:me"}}}))));
        assert_eq!(commands[0], Command::RemoteMedia("offer".into()));
        assert!(session.roster.is_empty());
        assert!(
            matches!(&commands[1], Command::Http { op: Op::Progress, body, .. } if body["callProgress"]["status"] == "ringing")
        );
        assert_eq!(session.phase, Phase::Notified);

        let media = Media {
            video: true,
            screen: false,
            content: payload::media_content("answer"),
        };
        let commands = session.handle(Input::Accept(media));
        let Command::Http {
            op: Op::Accept,
            url,
            body,
            ..
        } = &commands[0]
        else {
            panic!()
        };
        assert_eq!(url, "https://conv.example/accept");
        assert_eq!(
            body["callAcceptance"]["acceptedCallModalities"],
            json!(["Audio", "Video"])
        );
        assert_eq!(body["callAcceptance"]["mediaContent"]["blob"], "answer");
        assert_eq!(body["callAcceptance"]["acceptedBy"]["id"], "8:orgid:me");
        assert_eq!(session.phase, Phase::Connecting);

        let commands = session.handle(Input::Response(Op::Accept, Ok(json!({}))));
        assert!(commands.is_empty());
        assert_eq!(session.phase, Phase::Connecting);
        let ack = json!({"callAcceptanceAcknowledgement": {"links": {"mediaRenegotiation": "https://conv.example/reneg"}}});
        let commands = callback(&mut session, Scope::Call, "acknowledgement", ack.clone());
        assert!(commands.is_empty(), "do not acknowledge an acknowledgement");
        assert_eq!(session.phase, Phase::Connected);
        assert_eq!(
            session.link("mediaRenegotiation"),
            Some("https://conv.example/reneg")
        );
        assert!(
            callback(&mut session, Scope::Call, "acknowledgement", ack).is_empty(),
            "only once"
        );
        assert!(
            session.handle(Input::Timeout(Timer::Establish)).is_empty(),
            "answered: no longer missed"
        );
    }

    #[test]
    fn a_rejected_renegotiation_restores_the_media_and_is_retried_three_times() {
        let media = |video, blob| Media {
            video,
            screen: false,
            content: payload::media_content(blob),
        };
        let mut session = meeting(Some(media(false, "offer")));
        joined(&mut session);
        session.handle(Input::Response(
            Op::Join,
            Ok(json!({"links": {"mediaRenegotiation": "https://conv.example/reneg"}})),
        ));
        callback(&mut session, Scope::Call, "acceptance", json!({}));
        let reject = |session: &mut CallSession| {
            let body = json!({"mediaNegotiationFailure": {"code": 488}, "debugContent": {}});
            callback(session, Scope::Call, "mediaRejection", body)
        };
        assert!(
            reject(&mut session).is_empty(),
            "nothing offered: nothing to roll back"
        );

        for _ in 0..RENEGOTIATION_RETRIES {
            session.handle(Input::Renegotiate(media(true, "camera")));
            assert_eq!(
                reject(&mut session),
                [Command::LocalOfferRejected { retry: true }]
            );
            assert_eq!(session.media, Some(media(false, "offer")));
        }
        session.handle(Input::Renegotiate(media(true, "camera")));
        assert_eq!(
            reject(&mut session),
            [Command::LocalOfferRejected { retry: false }]
        );
        assert_eq!(session.media, Some(media(false, "offer")));

        // The count starts over, and an answered renegotiation resets it too.
        session.handle(Input::Renegotiate(media(true, "camera")));
        reject(&mut session);
        session.handle(Input::Renegotiate(media(true, "camera")));
        callback(
            &mut session,
            Scope::Call,
            "mediaAnswer",
            json!({"mediaAnswer": {"mediaContent": {"blob": "answer"},
                "links": {"mediaAcknowledgement": "https://conv.example/ack"}}}),
        );
        assert_eq!(session.media, Some(media(true, "camera")));
        for _ in 0..RENEGOTIATION_RETRIES {
            session.handle(Input::Renegotiate(media(true, "camera")));
            assert_eq!(
                reject(&mut session),
                [Command::LocalOfferRejected { retry: true }]
            );
        }

        // A 404 on the renegotiation link is not retried.
        session.handle(Input::Renegotiate(media(true, "camera")));
        assert_eq!(
            session.handle(Input::Response(Op::Renegotiate, Err(404))),
            [Command::LocalOfferRejected { retry: false }]
        );
    }

    #[test]
    fn renegotiates_both_ways_and_moves_callbacks_after_a_reconnect() {
        let mut session = meeting(Some(Media {
            video: false,
            screen: false,
            content: payload::media_content("offer"),
        }));
        joined(&mut session);
        session.handle(Input::Response(
            Op::Join,
            Ok(json!({"links": {"mediaRenegotiation": "https://conv.example/reneg"}})),
        ));
        // Not connected yet: nothing to renegotiate.
        let restart = Media {
            video: false,
            screen: false,
            content: payload::media_content("restart"),
        };
        assert!(
            session
                .handle(Input::Renegotiate(restart.clone()))
                .is_empty()
        );
        callback(&mut session, Scope::Call, "acceptance", json!({}));
        let commands = session.handle(Input::Renegotiate(restart.clone()));
        let Command::Http {
            op: Op::Renegotiate,
            url,
            body,
            ..
        } = &commands[0]
        else {
            panic!()
        };
        assert_eq!(url, "https://conv.example/reneg");
        assert_eq!(body["mediaNegotiation"]["mediaContent"]["blob"], "restart");
        assert!(
            body["mediaNegotiation"]["links"]["mediaAnswer"]
                .as_str()
                .unwrap()
                .ends_with("/call/mediaAnswer/r1")
        );
        let commands = session.handle(Input::Renegotiate(restart));
        assert!(
            matches!(&commands[0], Command::Http { body, .. } if body["mediaNegotiation"]["links"]["mediaAnswer"].as_str().unwrap().ends_with("/r2"))
        );

        let commands = callback(
            &mut session,
            Scope::Call,
            "mediaRenegotiation",
            json!({"mediaNegotiation": {
            "mediaContent": {"blob": "server-offer"},
            "links": {"mediaAnswer": "https://conv.example/answer"}}}),
        );
        assert_eq!(commands, [Command::RemoteOffer("server-offer".into())]);
        let commands = session.handle(Input::RenegotiationAnswer(Media {
            video: false,
            screen: false,
            content: payload::media_content("ours"),
        }));
        let Command::Http {
            op: Op::MediaAnswer,
            url,
            body,
            ..
        } = &commands[0]
        else {
            panic!()
        };
        assert_eq!(url, "https://conv.example/answer");
        assert_eq!(body["mediaAnswer"]["mediaContent"]["blob"], "ours");
        assert!(
            session
                .handle(Input::RenegotiationAnswer(Media {
                    video: false,
                    screen: false,
                    content: Value::Null
                }))
                .is_empty(),
            "answered once"
        );

        let commands = session.handle(Input::Rebase {
            surl: "https://t2.example/s2".into(),
            session: "sess2".into(),
        });
        let Command::Http {
            op: Op::KeepAlive,
            body,
            ..
        } = &commands[0]
        else {
            panic!()
        };
        assert!(
            body["links"]["conversationEnd"]
                .as_str()
                .unwrap()
                .starts_with("https://t2.example/s2/callAgent/sess2/call-1/")
        );
        assert_eq!(session.phase, Phase::Connected);
    }

    fn echo(chat: Option<&str>) -> CallSession {
        CallSession::new(
            "t".into(),
            local(),
            Callbacks::new("https://t.example/s", "sess", "t"),
            CallKind::EchoTest {
                bot: Target {
                    id: "28:echo".into(),
                    display_name: Some("Echo Bot".into()),
                },
                chat_thread: chat.map(str::to_owned),
            },
            Some(Media {
                video: true,
                screen: false,
                content: payload::media_content("offer"),
            }),
            "https://conv.example/epconv/".into(),
        )
    }

    #[test]
    fn test_calls_start_without_ringing_then_add_the_echo_bot() {
        let mut session = echo(Some("19:bot.gbl.spaces"));
        let commands = session.handle(Input::Begin);
        let Command::Http {
            op: Op::Join, body, ..
        } = &commands[0]
        else {
            panic!()
        };
        assert!(body["participants"].get("to").is_none());
        assert_eq!(body["conversationRequest"]["suppressDialout"], true);
        assert_eq!(body["groupChat"]["threadId"], "19:bot.gbl.spaces");
        assert_eq!(
            body["callInvitation"]["callModalities"],
            json!(["Audio", "Video"])
        );
        let commands = session.handle(Input::Response(
            Op::Start,
            Ok(json!({"links": {
            "addParticipant": "https://conv.example/add", "leave": "https://conv.example/leave"}})),
        ));
        assert!(
            !commands.iter().any(|c| matches!(
                c,
                Command::Http {
                    op: Op::AddParticipant,
                    ..
                }
            )),
            "201 does not mean the call is connected"
        );
        let commands = callback(&mut session, Scope::Call, "acceptance", json!({}));
        let Command::Http {
            op: Op::AddParticipant,
            url,
            body,
            ..
        } = &commands[0]
        else {
            panic!()
        };
        assert_eq!(url, "https://conv.example/add");
        assert_eq!(body["participants"]["to"][0]["id"], "28:echo");
        assert!(body["links"]["addParticipantSuccess"].is_string());
        assert_eq!(session.phase, Phase::Connected);
        assert!(
            callback(&mut session, Scope::Call, "acceptance", json!({})).is_empty(),
            "add the bot only once"
        );
        callback(
            &mut session,
            Scope::Conversation,
            "rosterUpdate",
            json!({"participants": {"28:echo": {"displayName": "Echo Bot"}}}),
        );
        assert_eq!(session.phase, Phase::Connected);

        let mut failed = echo(Some("19:bot.gbl.spaces"));
        failed.handle(Input::Begin);
        failed.handle(Input::Response(
            Op::Start,
            Ok(json!({"links": {"leave": "https://conv.example/leave"}})),
        ));
        callback(&mut failed, Scope::Call, "acceptance", json!({}));
        assert_eq!(failed.phase, Phase::Ending, "no way to add the bot: leave");
        failed.handle(Input::Response(Op::Leave, Ok(Value::Null)));
        assert!(matches!(&failed.phase, Phase::Ended(r) if r.message().contains("Echo Bot")));
    }

    #[test]
    fn without_a_bot_chat_the_test_call_still_adds_the_bot_after_start() {
        let mut session = echo(None);
        let commands = session.handle(Input::Begin);
        let Command::Http {
            op: Op::Join, body, ..
        } = &commands[0]
        else {
            panic!()
        };
        assert!(body["participants"].get("to").is_none());
        let commands = session.handle(Input::Response(
            Op::Start,
            Ok(json!({"links": {
                "addParticipant": "https://conv.example/add"
            }})),
        ));
        assert!(!commands.iter().any(|c| matches!(
            c,
            Command::Http {
                op: Op::AddParticipant,
                ..
            }
        )));
        let commands = callback(&mut session, Scope::Call, "acceptance", json!({}));
        assert!(commands.iter().any(|c| matches!(
            c,
            Command::Http {
                op: Op::AddParticipant,
                ..
            }
        )));
    }

    #[test]
    fn unanswered_or_cancelled_incoming_calls_are_missed() {
        let mut session = incoming();
        session.handle(Input::Begin);
        session.handle(Input::Timeout(Timer::Establish));
        assert!(matches!(&session.phase, Phase::Ended(r) if r.message() == "Appel manqué."));

        let mut session = incoming();
        session.handle(Input::Begin);
        callback(
            &mut session,
            Scope::Call,
            "end",
            json!({"callEnd": {"code": 487, "phrase": "CallerCancelled"}}),
        );
        assert!(
            matches!(&session.phase, Phase::Ended(r) if !r.local && r.message() == "Appel manqué.")
        );
    }

    #[test]
    fn incoming_calls_attach_hand_over_the_offer_and_can_be_declined() {
        let call = IncomingCall {
            event_id: 107,
            caller_id: "8:orgid:bob".into(),
            caller_name: Some("Bob".into()),
            attach: "https://conv.example/attach".into(),
            conversation_controller: Some("https://conv.example/c1".into()),
            thread_id: None,
            multiparty: false,
            call_id: None,
        };
        let mut session = CallSession::new(
            "c".into(),
            local(),
            Callbacks::new("https://t.example/s", "sess", "c"),
            CallKind::Incoming(call),
            None,
            String::new(),
        );
        assert_eq!(session.phase, Phase::Notified);
        let commands = session.handle(Input::Begin);
        assert!(
            matches!(&commands[0], Command::Http { op: Op::Attach, url, .. } if url == "https://conv.example/attach")
        );
        assert!(commands.contains(&Command::Timer(Timer::Establish, INCOMING_TIMEOUT)));
        let commands = session.handle(Input::Response(Op::Attach, Ok(json!({
            "callInvitation": {"mediaContent": {"blob": "offer"},
                "links": {"callLeg": "https://conv.example/reject", "acceptance": "https://conv.example/accept"}}}))));
        assert_eq!(commands, [Command::RemoteMedia("offer".into())]);
        let commands = session.handle(Input::Decline);
        assert!(
            matches!(&commands[0], Command::Http { op: Op::Reject, method, body, .. }
            if *method == Method::DELETE && body["callEnd"]["code"] == 603)
        );
        session.handle(Input::Response(Op::Reject, Ok(Value::Null)));
        assert!(matches!(&session.phase, Phase::Ended(r) if r.code == 603));
    }

    fn recording_request() -> payload::recording::Request {
        payload::recording::Request {
            token: "aad".into(),
            title: "Point hebdo".into(),
            organizer: "Organisatrice".into(),
            ical_uid: None,
            file_name: "Point hebdo-20261030_101500-Meeting Recording".into(),
            correlation_id: "corr".into(),
        }
    }

    fn recorder_roster(state: &str, by: Option<&str>) -> Value {
        json!({"participants": {payload::recording::BOT_MRIS[0]: {
            "displayName": "Recorder",
            "endpoints": {"bot": {"call": {}, "endpointMetadata": {
                "commandUrl": "https://bot.example/command",
                "processingModes": {"recording": {"state": state, "userId": by}},
            }}},
        }}})
    }

    fn with_add_links(session: &mut CallSession) {
        callback(
            session,
            Scope::Conversation,
            "conversationUpdate",
            json!({"links": {"addParticipant": "https://conv.example/c1/add",
                             "addParticipantAndModality": "https://conv.example/c1/addm"}}),
        );
    }

    #[test]
    fn recording_invites_the_bot_then_starts_it_once_it_is_in_the_roster() {
        let mut session = meeting(None);
        joined(&mut session);
        with_add_links(&mut session);
        let commands = session.handle(Input::StartRecording(recording_request()));
        let [Command::Http { op, url, body, .. }] = commands.as_slice() else {
            panic!("one invite: {commands:?}");
        };
        assert_eq!(*op, Op::RecorderInvite);
        assert_eq!(url, "https://conv.example/c1/addm", "meetings use addParticipantAndModality");
        assert_eq!(body.pointer("/participants/to/0/id"), Some(&json!(payload::recording::BOT_MRIS[0])));
        assert_eq!(body.pointer("/participantInvitationData/botData/mode"), Some(&json!("Recording")));
        assert_eq!(body.pointer("/participantInvitationData/botData/initiatorUserToken"), Some(&json!("aad")));
        assert_eq!(body.pointer("/groupChat/threadId"), Some(&json!("19:meeting_x@thread.v2")));

        let commands = callback(&mut session, Scope::Conversation, "rosterUpdate", recorder_roster("Inactive", None));
        let starts: Vec<_> = commands
            .iter()
            .filter_map(|c| match c {
                Command::Http { op: Op::RecorderCommand(true), url, body, .. } => Some((url, body)),
                _ => None,
            })
            .collect();
        let [(url, body)] = starts.as_slice() else { panic!("one start: {commands:?}") };
        assert_eq!(*url, "https://bot.example/command");
        assert_eq!(body["action"], "start");
        assert_eq!(body["processingModes"], json!(["recording"]));
        assert_eq!(body["participantLegId"], "p");
        assert_eq!(
            body.pointer("/actionParameters/recordingStorageSettings/0/StorageType"),
            Some(&json!("OnedriveForBusiness"))
        );
        assert_eq!(session.recording, Recording::Starting);

        // The same roster again does not start it twice.
        let again = callback(&mut session, Scope::Conversation, "rosterUpdate", recorder_roster("Inactive", None));
        assert!(!again.iter().any(|c| matches!(c, Command::Http { op: Op::RecorderCommand(_), .. })));

        callback(&mut session, Scope::Conversation, "rosterUpdate", recorder_roster("Active", Some("8:orgid:me")));
        assert_eq!(session.recording, Recording::Idle);
        let recorder = session.recorder().expect("bot in the roster");
        assert_eq!(recorder.state, RecorderState::Active);
        assert_eq!(recorder.user_id.as_deref(), Some("8:orgid:me"));
    }

    #[test]
    fn stopping_sends_stop_to_the_bot_and_settles_when_it_reports_inactive() {
        let mut session = meeting(None);
        joined(&mut session);
        callback(&mut session, Scope::Conversation, "rosterUpdate", recorder_roster("Active", Some("8:orgid:bob")));
        let commands = session.handle(Input::StopRecording);
        let [Command::Http { op: Op::RecorderCommand(false), body, .. }] = commands.as_slice() else {
            panic!("one stop: {commands:?}");
        };
        assert_eq!(body["action"], "stop");
        assert!(body.get("actionParameters").is_none());
        assert_eq!(session.recording, Recording::Stopping);
        callback(&mut session, Scope::Conversation, "rosterUpdate", recorder_roster("Inactive", Some("8:orgid:me")));
        assert_eq!(session.recording, Recording::Idle);
    }

    #[test]
    fn a_recorder_already_in_the_call_is_started_without_inviting_it_again() {
        let mut session = meeting(None);
        joined(&mut session);
        with_add_links(&mut session);
        callback(&mut session, Scope::Conversation, "rosterUpdate", recorder_roster("Inactive", None));
        let commands = session.handle(Input::StartRecording(recording_request()));
        assert!(matches!(commands.as_slice(), [Command::Http { op: Op::RecorderCommand(true), .. }]));
    }

    #[test]
    fn a_refused_invite_marks_the_recording_failed() {
        let mut session = meeting(None);
        joined(&mut session);
        with_add_links(&mut session);
        session.handle(Input::StartRecording(recording_request()));
        callback(&mut session, Scope::Conversation, "addParticipantFailure", json!({"code": 403}));
        assert_eq!(session.recording, Recording::Failed);
        assert!(!session.ended(), "a failed recording does not end the meeting");
    }

    #[test]
    fn recorder_metadata_may_come_as_a_json_string() {
        let mut session = meeting(None);
        joined(&mut session);
        let metadata = json!({"commandUrl": "https://bot.example/c", "processingModes": {"recording": {"state": "Active"}}});
        callback(
            &mut session,
            Scope::Conversation,
            "rosterUpdate",
            json!({"participants": {payload::recording::BOT_MRIS[1]: {
                "endpoints": {"bot": {"call": {}, "endpointMetadata": metadata.to_string()}},
            }}}),
        );
        assert_eq!(session.recorder().map(|r| r.state), Some(RecorderState::Active));
    }
}

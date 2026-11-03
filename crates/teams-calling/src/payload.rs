//! Request bodies of the NGC conversation service (only the inner `payload` goes on the wire).
use serde_json::{Value, json};

use crate::MeetingCoordinates;

/// GVC join (1), cloud screen sharing (2, else 426 from meetings hosted elsewhere), hostless calls (4), base bits.
pub const ENDPOINT_CAPABILITIES: u64 = 1 | 2 | 4 | 64 | 1024;

/// The local participant, as every command names it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Local {
    /// `8:orgid:<oid>`.
    pub id: String,
    pub display_name: String,
    /// Same as the Trouter registration id.
    pub endpoint_id: String,
    /// One per call.
    pub participant_id: String,
    pub language_id: String,
}

impl Local {
    pub fn from(&self) -> Value {
        json!({
            "id": self.id,
            "displayName": self.display_name,
            "endpointId": self.endpoint_id,
            "participantId": self.participant_id,
            "languageId": self.language_id,
        })
    }
}

/// Where the service POSTs callbacks of one call: `{surl}/callAgent/{session}/{call}`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Callbacks {
    pub base: String,
}

impl Callbacks {
    pub fn new(surl: &str, session: &str, call: &str) -> Self {
        Self {
            base: format!("{}/callAgent/{session}/{call}", surl.trim_end_matches('/')),
        }
    }
    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }
    fn roster(&self) -> Value {
        json!({"type": "Delta", "rosterUpdate": self.url("/conversation/rosterUpdate/")})
    }

    fn conversation_links(&self) -> Value {
        json!({
            "conversationEnd": self.url("/conversation/conversationEnd/"),
            "conversationUpdate": self.url("/conversation/conversationUpdate/"),
            "localParticipantUpdate": self.url("/conversation/localParticipantUpdate/"),
            "addParticipantSuccess": self.url("/conversation/addParticipantSuccess/"),
            "addParticipantFailure": self.url("/conversation/addParticipantFailure/"),
            "receiveMessage": self.url("/conversation/receiveMessage/"),
        })
    }

    fn call_links(&self) -> Value {
        json!({
            "progress": self.url("/call/progress/"),
            "mediaAnswer": self.url("/call/mediaAnswer/"),
            "acceptance": self.url("/call/acceptance/"),
            "redirection": self.url("/call/redirection/"),
            "end": self.url("/call/end/"),
        })
    }
}

/// `{contentType:"application/sdp-ngc-1.0", blob}`.
pub fn media_content(blob: &str) -> Value {
    json!({"contentType": "application/sdp-ngc-1.0", "blob": blob})
}

fn modalities(media: &Media) -> Value {
    let mut list = vec!["Audio"];
    if media.video {
        list.push("Video");
    }
    if media.screen {
        list.push("ScreenSharer");
    }
    json!(list)
}

fn targets(to: &[Target]) -> Vec<Value> {
    to.iter()
        .map(|t| json!({"id": t.id, "displayName": t.display_name}))
        .collect()
}

fn conversation_request(callbacks: &Callbacks, subject: Option<&str>) -> Value {
    json!({
        "subject": subject,
        "roster": callbacks.roster(),
        "properties": {
            "allowConversationWithoutHost": true,
            "enableGroupCallEventMessages": true,
            "enableGroupCallUpgradeMessage": true,
            "enableGroupCallMeetupGeneration": false,
        },
        "links": callbacks.conversation_links(),
    })
}

fn call_invitation(callbacks: &Callbacks, media: &Media) -> Value {
    json!({
        "replaces": null,
        "transferor": null,
        "links": callbacks.call_links(),
        "callModalities": modalities(media),
        "clientContentForMediaController": {
            "controlVideoStreaming": callbacks.url("/call/controlVideoStreaming/"),
            "csrcInfo": callbacks.url("/call/csrcInfo/"),
        },
        "mediaContent": media.content,
    })
}

/// Media of a join or call: `None` subscribes to the roster only (`subscribe:true`).
#[derive(Debug, Clone, PartialEq)]
pub struct Media {
    pub video: bool,
    /// We share our screen (`ScreenSharer` modality).
    pub screen: bool,
    /// `mediaContent` with the local NGC offer.
    pub content: Value,
}

/// `JoinConversation` for a scheduled meeting or an existing group call.
pub fn join(
    local: &Local,
    callbacks: &Callbacks,
    meeting: &MeetingCoordinates,
    media: Option<&Media>,
) -> Value {
    let mut request = conversation_request(callbacks, None);
    request["suppressDialout"] = json!(true);
    if media.is_none() {
        // Roster-only join: the split conversationEnd URL of the subscribe flavour is optional.
        request["links"]["conversationEnd"] =
            json!(callbacks.url("/conversation/conversationEnd/"));
    }
    let mut body = json!({
        "conversationRequest": request,
        "groupContext": null,
        "groupChat": {"threadId": meeting.thread_id, "messageId": meeting.message_id},
        "participants": {"from": local.from()},
        "capabilities": null,
        "endpointCapabilities": ENDPOINT_CAPABILITIES,
        "clientEndpointCapabilities": null,
        "meetingInfo": {
            "tenantId": meeting.tenant_id,
            "organizerId": meeting.organizer_id,
        },
    });
    match media {
        Some(media) => body["callInvitation"] = call_invitation(callbacks, media),
        None => body["subscribe"] = json!(true),
    }
    body
}

/// Someone to call: an MRI (`8:orgid:…`, `4:+33…`) and an optional display name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub id: String,
    pub display_name: Option<String>,
}

/// `StartCall`: a 1:1 call, or a hostless group call tied to a chat.
pub fn start_call(
    local: &Local,
    callbacks: &Callbacks,
    to: &[Target],
    chat_thread: Option<&str>,
    media: &Media,
) -> Value {
    let mut request = conversation_request(callbacks, None);
    request["suppressDialout"] = json!(false);
    request["links"]["confirmUnmute"] = json!(callbacks.url("/conversation/confirmUnmute/"));
    json!({
        "conversationRequest": request,
        "groupChat": chat_thread.map(|id| json!({"threadId": id, "messageId": null})),
        "participants": {"from": local.from(), "to": targets(to)},
        "capabilities": null,
        "endpointCapabilities": ENDPOINT_CAPABILITIES,
        "clientEndpointCapabilities": null,
        "callInvitation": call_invitation(callbacks, media),
    })
}

/// JoinConversation for a group call that rings nobody (`participants.to:[]` would select StartCall).
pub fn join_group_call(
    local: &Local,
    callbacks: &Callbacks,
    chat_thread: Option<&str>,
    media: &Media,
) -> Value {
    let mut body = start_call(local, callbacks, &[], chat_thread, media);
    body["participants"].as_object_mut().unwrap().remove("to");
    body["conversationRequest"]["suppressDialout"] = json!(true);
    body["groupContext"] = Value::Null;
    body["meetingInfo"] = Value::Null;
    body
}

/// Incoming call acknowledgement: attach, and join the conversation in the same request.
pub fn attach(
    local: &Local,
    callbacks: &Callbacks,
    conversation_controller: &str,
    require_media: bool,
) -> Value {
    json!({
        "attach": {
            "requireMediaContent": require_media,
            "links": {"end": callbacks.url("/call/end/")},
        },
        "capabilities": null,
        "endpointCapabilities": ENDPOINT_CAPABILITIES,
        "additionalActions": [{
            "input": {
                "capabilities": null,
                "endpointCapabilities": ENDPOINT_CAPABILITIES,
                "conversationRequest": {
                    "roster": callbacks.roster(),
                    "links": callbacks.conversation_links(),
                },
                "participants": {"from": local.from()},
            },
            "name": "join",
            "url": conversation_controller,
            "waitForResponse": true,
        }],
    })
}

fn acceptance_links(callbacks: &Callbacks) -> Value {
    json!({
        "mediaRenegotiation": callbacks.url("/call/mediaRenegotiation/"),
        "transfer": callbacks.url("/call/transfer/"),
        "replacement": callbacks.url("/call/replacement/"),
        "balanceUpdate": callbacks.url("/call/balanceUpdate/"),
        "retargetCompletion": callbacks.url("/call/retargetCompletion/"),
        "controlVideoStreaming": callbacks.url("/call/controlVideoStreaming/"),
        "updateMediaDescriptions": callbacks.url("/call/updateMediaDescriptions"),
    })
}

/// Answering an incoming call: `POST` on the accept link, with our media answer.
pub fn accept(local: &Local, callbacks: &Callbacks, media: &Media) -> Value {
    json!({"callAcceptance": {
        "acceptedBy": local.from(),
        "acceptedCallModalities": modalities(media),
        "capabilities": null,
        "endpointCapabilities": ENDPOINT_CAPABILITIES,
        "clientEndpointCapabilities": null,
        "links": acceptance_links(callbacks),
        "mediaContent": media.content,
        "callKeepAliveInterval": null,
    }})
}

/// After the service acknowledged our acceptance.
pub fn acceptance_acknowledgement(callbacks: &Callbacks) -> Value {
    json!({"callAcceptanceAcknowledgement": {"links": acceptance_links(callbacks)}})
}

/// New local offer during a call: ICE restart, camera added. `tag` keeps answers apart.
pub fn renegotiation(local: &Local, callbacks: &Callbacks, media: &Media, tag: &str) -> Value {
    json!({"mediaNegotiation": {
        "callModalities": modalities(media),
        "sender": local.from(),
        "links": {
            "mediaAnswer": callbacks.url(&format!("/call/mediaAnswer/{tag}")),
            "rejection": callbacks.url(&format!("/call/mediaRejection/{tag}")),
        },
        "mediaContent": media.content,
    }})
}

/// Our answer to the service's renegotiation.
pub fn media_answer(local: &Local, callbacks: &Callbacks, media: &Media) -> Value {
    json!({"mediaAnswer": {
        "callModalities": modalities(media),
        "sender": local.from(),
        "links": {"mediaAcknowledgement": callbacks.url("/call/mediaAcknowledgement/")},
        "mediaContent": media.content,
    }})
}

/// Adding people to the call: `POST` on `links.addParticipant`.
pub fn add_participant(
    local: &Local,
    callbacks: &Callbacks,
    to: &[Target],
    chat_thread: Option<&str>,
) -> Value {
    json!({
        "participants": {"from": local.from(), "to": targets(to)},
        "groupContext": null,
        "groupChat": chat_thread.map(|id| json!({"threadId": id, "messageId": null})),
        "links": {
            "addParticipantSuccess": callbacks.url("/conversation/addParticipantSuccess/"),
            "addParticipantFailure": callbacks.url("/conversation/addParticipantFailure/"),
        },
    })
}

/// Tells the caller this endpoint is ringing.
pub fn ringing(local: &Local) -> Value {
    json!({"callProgress": {"sender": local.from(), "status": "ringing", "phrase": "ringing"}})
}

/// End of a call leg, as `callEnd` / `callTransactionEnd` carry it.
pub fn call_end(code: u16, sub_code: u32, phrase: &str) -> Value {
    json!({"code": code, "subCode": sub_code, "phrase": phrase})
}

/// Declining an incoming call: `DELETE` on the reject link, code 603.
pub fn reject() -> Value {
    json!({"callEnd": call_end(603, 0, "Declined")})
}

/// Leaving: `POST` on `links.leave`.
pub fn leave(local: &Local) -> Value {
    json!({
        "participants": {"from": local.from()},
        "conversationTransactionEnd": {"reason": "noError", "code": 0, "phrase": "ConversationEndNoModality"},
        "callTransactionEnd": call_end(0, 0, "LocalUserEnded"),
    })
}

/// Ending the conversation for everyone: `DELETE` on the conversation controller.
pub fn end_for_everyone(local: &Local, call_end: Value) -> Value {
    let mut body = leave(local);
    body["conversationTransactionEnd"]["phrase"] = json!("ConversationEndForAllInitiated");
    body["callTransactionEnd"] = call_end;
    body
}

/// Removing our own other devices: `POST` on `links.removeParticipant`.
pub fn remove_endpoints(local: &Local, callbacks: &Callbacks, endpoints: &[String]) -> Value {
    let to: Vec<Value> = endpoints
        .iter()
        .map(|e| json!({"id": local.id, "endpointId": e}))
        .collect();
    json!({
        "participants": {"from": local.from(), "toEndpoints": to},
        "links": {
            "removeParticipantSuccess": callbacks.url("/conversation/removeParticipantSuccess/"),
            "removeParticipantFailure": callbacks.url("/conversation/removeParticipantFailure/"),
        },
    })
}

/// `getSignalingSourceRequestMessage`: the control message goes JSON-encoded in `mediaParameter`.
pub fn source_request(
    slot: &crate::sdp::VideoSlot,
    source_id: u32,
    cameras: usize,
    sequence: u32,
) -> Value {
    let (fs, mbps, fps) = slot.receive_limits(cameras);
    let control = json!({"controlVideoStreaming": {
        "sequenceNumber": sequence,
        "controlInfo": {
            "sourceId": source_id,
            "streamMsid": slot.stream_id,
            "fmtParams": [{"max-fs": fs, "max-mbps": mbps, "max-fps": fps, "profile-level-id": "64001f"}],
        },
    }});
    json!({"applyChannelParameters": {"multiChannelParameter": {
        "mids": [slot.mid],
        "mediaParameter": control.to_string(),
    }}})
}

/// Muting or unmuting oneself.
pub fn mute(local: &Local, muted: bool) -> Value {
    if muted {
        json!({"from": local.from(), "scope": "Myself", "mediaTypes": ["audio"]})
    } else {
        json!({"from": local.from(), "mediaTypes": ["audio"]})
    }
}

/// Renewing the callback links of a conversation (keep-alive every ~45 min).
pub fn notification_links(local: &Local, callbacks: &Callbacks) -> Value {
    json!({
        "participants": {"from": local.from()},
        "roster": callbacks.roster(),
        "links": callbacks.conversation_links(),
    })
}

/// Cloud recording, as the web client's `callRecording` config and `CallingBotRestClient`.
pub mod recording {
    use super::*;

    /// `recordingBotMri`, then `recordingBotMrisFromAlternateRegions`.
    pub const BOT_MRIS: [&str; 2] = [
        "28:bdd75849-e0a6-4cce-8fc1-d7c0d4da43e5",
        "28:4b25d9f8-18f5-4d09-a582-cd0a28f63181",
    ];

    pub fn is_bot(mri: &str) -> bool {
        BOT_MRIS.iter().any(|bot| bot.eq_ignore_ascii_case(mri))
    }

    /// What the recorder needs to know about the meeting, and the initiator's token for it.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Request {
        /// AAD token for `callRecording.resource` (`initiatorUserToken`).
        pub token: String,
        pub title: String,
        /// Display name of the organizer (`extendRecordingOrganzierPayloadInfo`).
        pub organizer: String,
        pub ical_uid: Option<String>,
        /// `<title>-<local date time>-Meeting Recording`, as `getRecordingFileName`.
        pub file_name: String,
        pub correlation_id: String,
    }

    fn storage(request: &Request) -> Value {
        json!([{
            "StorageType": "OnedriveForBusiness",
            "StorageLocation": "Recordings",
            "FileName": request.file_name,
        }])
    }

    /// `addParticipant(recordingBotMri, {participantInvitationData: {botData}})`.
    pub fn invite(
        local: &Local,
        callbacks: &Callbacks,
        call_id: &str,
        chat_thread: Option<&str>,
        request: &Request,
    ) -> Value {
        let mut body = add_participant(
            local,
            callbacks,
            &[Target {
                id: BOT_MRIS[0].into(),
                display_name: None,
            }],
            chat_thread,
        );
        body["disableUnmute"] = json!(false);
        body["participantInvitationData"] = json!({"botData": {
            "meetingTitle": request.title,
            "clientInfo": "Teams-general",
            "callId": call_id,
            "threadId": chat_thread,
            "mode": "Recording",
            "iCalUid": request.ical_uid,
            "consumerType": "Teams",
            "correlationId": request.correlation_id,
            "initiatorUserToken": request.token,
            "meetingOrganizer": request.organizer,
            "recordingStorageSettings": storage(request),
        }});
        body
    }

    /// `sendCommand` to the bot's `commandUrl`: `start` with storage parameters, or `stop`.
    pub fn command(local: &Local, start: Option<&Request>) -> Value {
        let mut body = json!({
            "timestamp": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            "participantMri": local.id,
            "participantLegId": local.participant_id,
            "action": if start.is_some() { "start" } else { "stop" },
            "mode": "recording",
            "processingModes": ["recording"],
            "participantSkypeToken": "",
        });
        if let Some(request) = start {
            body["actionParameters"] = json!({
                "recordingStorageSettings": storage(request),
                "iCalUid": request.ical_uid,
                "correlationId": request.correlation_id,
                "meetingTitle": request.title,
                "meetingOrganizer": request.organizer,
                "type": "start",
            });
        }
        body
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local() -> Local {
        Local {
            id: "8:orgid:me".into(),
            display_name: "Moi".into(),
            endpoint_id: "ep".into(),
            participant_id: "p".into(),
            language_id: "fr-fr".into(),
        }
    }
    fn meeting() -> MeetingCoordinates {
        MeetingCoordinates {
            thread_id: "19:meeting_x@thread.v2".into(),
            message_id: "0".into(),
            tenant_id: Some("t".into()),
            organizer_id: Some("o".into()),
        }
    }

    #[test]
    fn sharing_our_screen_is_announced_in_the_renegotiation() {
        let callbacks = Callbacks::new("https://t.example/s", "sess", "c");
        let media = Media {
            video: true,
            screen: true,
            content: media_content("offer"),
        };
        let body = renegotiation(&local(), &callbacks, &media, "r1");
        assert_eq!(
            body["mediaNegotiation"]["callModalities"],
            json!(["Audio", "Video", "ScreenSharer"])
        );
    }

    #[test]
    fn roster_only_join_subscribes_without_a_call_invitation() {
        let callbacks = Callbacks::new("https://t.example:3443/s/", "sess", "call");
        let body = join(&local(), &callbacks, &meeting(), None);
        assert_eq!(body["subscribe"], true);
        assert!(body.get("callInvitation").is_none());
        assert_eq!(body["groupChat"]["threadId"], "19:meeting_x@thread.v2");
        assert_eq!(body["meetingInfo"]["tenantId"], "t");
        assert_eq!(body["meetingInfo"]["organizerId"], "o");
        assert_eq!(body["participants"]["from"]["endpointId"], "ep");
        assert_eq!(body["endpointCapabilities"], 1095);
        assert_eq!(
            body["conversationRequest"]["roster"]["rosterUpdate"],
            "https://t.example:3443/s/callAgent/sess/call/conversation/rosterUpdate/"
        );
        assert_eq!(body["conversationRequest"]["suppressDialout"], true);
    }

    #[test]
    fn media_joins_and_calls_carry_modalities_and_the_ngc_offer() {
        let callbacks = Callbacks::new("https://t.example/s", "sess", "call");
        let media = Media {
            video: true,
            screen: false,
            content: media_content("v=0"),
        };
        let body = join(&local(), &callbacks, &meeting(), Some(&media));
        assert!(body.get("subscribe").is_none());
        assert_eq!(
            body["callInvitation"]["callModalities"],
            json!(["Audio", "Video"])
        );
        assert_eq!(
            body["callInvitation"]["mediaContent"]["contentType"],
            "application/sdp-ngc-1.0"
        );
        assert_eq!(
            body["callInvitation"]["links"]["mediaAnswer"],
            "https://t.example/s/callAgent/sess/call/call/mediaAnswer/"
        );
        let call = start_call(
            &local(),
            &callbacks,
            &[Target {
                id: "8:orgid:bob".into(),
                display_name: None,
            }],
            Some("19:chat@thread.v2"),
            &Media {
                video: false,
                screen: false,
                content: media_content("v=0"),
            },
        );
        assert_eq!(call["participants"]["to"][0]["id"], "8:orgid:bob");
        assert_eq!(call["callInvitation"]["callModalities"], json!(["Audio"]));
        assert_eq!(call["groupChat"]["threadId"], "19:chat@thread.v2");
    }

    #[test]
    fn attach_reject_leave_and_mute_bodies() {
        let callbacks = Callbacks::new("https://t.example/s/", "sess", "call");
        let body = attach(&local(), &callbacks, "https://conv.example/c1", true);
        assert_eq!(body["attach"]["requireMediaContent"], true);
        assert_eq!(body["additionalActions"][0]["name"], "join");
        assert_eq!(
            body["additionalActions"][0]["url"],
            "https://conv.example/c1"
        );
        assert_eq!(reject()["callEnd"]["code"], 603);
        let leave = leave(&local());
        assert_eq!(leave["conversationTransactionEnd"]["reason"], "noError");
        assert_eq!(leave["callTransactionEnd"]["code"], 0);
        assert_eq!(mute(&local(), true)["scope"], "Myself");
        assert!(mute(&local(), false).get("scope").is_none());
    }
}

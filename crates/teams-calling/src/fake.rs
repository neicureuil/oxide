//! In-process conversation service for tests and demo mode, delivering callbacks through a [`FakeTrouter`].
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use axum::extract::{Path, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{Value, json};
use teams_realtime::fake::FakeTrouter;
use url::Url;

#[derive(Debug, Default, Clone)]
pub struct Seen {
    pub joins: Vec<Value>,
    /// Headers of each join, lower-case names.
    pub join_headers: Vec<BTreeMap<String, String>>,
    pub leaves: Vec<Value>,
    pub mutes: Vec<(String, Value)>,
    pub keep_alives: u32,
    pub attaches: Vec<Value>,
    /// Ids added with `addParticipant`.
    pub added: Vec<String>,
    pub accepts: Vec<Value>,
    pub rejects: u32,
    pub ringing: u32,
    pub acknowledgements: u32,
    /// Bodies sent to the recorder bot's `commandUrl`, with their `authorization` header.
    pub recorder_commands: Vec<(Value, Option<String>)>,
}

struct Inner {
    base: Url,
    trouter: FakeTrouter,
    lobby: bool,
    seen: Mutex<Seen>,
    /// Callback links of the latest join.
    links: Mutex<Value>,
    sequence: Mutex<i64>,
    /// Body of the latest attach of an incoming call.
    attach: Mutex<Value>,
}

#[derive(Clone)]
pub struct FakeConversation(Arc<Inner>);

impl FakeConversation {
    /// `lobby`: joiners wait in the lobby until [`FakeConversation::admit`].
    pub async fn start(trouter: FakeTrouter, lobby: bool) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind fake conversation service");
        let addr = listener.local_addr().expect("fake conversation address");
        let fake = Self(Arc::new(Inner {
            base: Url::parse(&format!("http://{addr}/")).unwrap(),
            trouter,
            lobby,
            seen: Mutex::default(),
            links: Mutex::new(Value::Null),
            sequence: Mutex::new(1),
            attach: Mutex::new(Value::Null),
        }));
        let app = Router::new()
            .route("/epconv/", post(join))
            .route("/redirected/", post(join))
            .route("/c/{id}/{op}", post(operation))
            .route("/incoming/{op}", post(incoming).delete(reject))
            .with_state(fake.clone());
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        fake
    }

    /// Value of `calling_conversationServiceUrl`.
    pub fn url(&self) -> Url {
        self.0.base.join("epconv/").unwrap()
    }
    pub fn seen(&self) -> Seen {
        self.0.seen.lock().unwrap().clone()
    }

    fn incoming_link(&self, op: &str) -> String {
        self.0
            .base
            .join(&format!("incoming/{op}"))
            .unwrap()
            .to_string()
    }

    fn next_sequence(&self) -> i64 {
        let mut sequence = self.0.sequence.lock().unwrap();
        *sequence += 1;
        *sequence
    }

    /// POSTs `body` to a callback URL, through Trouter.
    pub fn callback(&self, url: &str, body: Value) {
        let surl = self.0.trouter.surl();
        let Some(path) = url.strip_prefix(surl.trim_end_matches('/')) else {
            return;
        };
        self.0.trouter.push(
            "POST",
            path,
            json!({"Content-Type": "application/json", "MS-CV": "fake"}),
            &body.to_string(),
        );
    }

    fn link(&self, pointer: &str) -> Option<String> {
        self.0
            .links
            .lock()
            .unwrap()
            .pointer(pointer)
            .and_then(Value::as_str)
            .map(str::to_owned)
    }

    fn media_blob(&self) -> Option<String> {
        self.link("/callInvitation/mediaContent/blob")
    }

    /// Someone calls the client: an `eventId` 107 notification pushed through Trouter.
    pub fn ring_me(&self, caller: &str, name: &str, video: bool) {
        let mut offer = String::from(
            "v=0\r\no=- 1 1 IN IP4 127.0.0.1\r\ns=-\r\nt=0 0\r\nm=audio 9 RTP/SAVP 111\r\nc=IN IP4 127.0.0.1\r\na=x-ssrc-range:5000-5000\r\na=ice-ufrag:u\r\na=ice-pwd:p\r\na=fingerprint:sha-256 AA:BB\r\na=setup:actpass\r\na=mid:0\r\na=sendrecv\r\na=rtcp-mux\r\na=rtpmap:111 opus/48000/2\r\na=label:main-audio\r\n",
        );
        if video {
            offer.push_str("m=video 9 RTP/SAVP 102\r\nc=IN IP4 127.0.0.1\r\na=x-ssrc-range:6000-6099\r\na=mid:1\r\na=sendrecv\r\na=rtcp-mux\r\na=rtpmap:102 H264/90000\r\na=label:main-video\r\n");
        }
        // Keep the attach of a call already ringing, so it can still be cancelled.
        self.0.attach.lock().unwrap()["offer"] = json!(offer);
        let notification = json!({
            "callNotification": {
                "from": {"id": caller, "displayName": name},
                "links": {"attach": self.incoming_link("attach")},
            },
            "conversationInvitation": {"conversationController": self.0.base.join("c/incoming/").unwrap().as_str(), "isMultiParty": false},
            "debugContent": {"callId": "fake-incoming"},
        });
        self.0.trouter.push(
            "POST",
            "/incoming-call",
            json!({"Content-Type": "application/json"}),
            &json!({"eventId": 107, "gp": notification}).to_string(),
        );
    }

    /// The caller gives up before anyone answers.
    pub fn cancel_incoming(&self) {
        let end = self
            .0
            .attach
            .lock()
            .unwrap()
            .pointer("/attach/links/end")
            .and_then(Value::as_str)
            .map(str::to_owned);
        if let Some(url) = end {
            self.callback(
                &url,
                json!({"callEnd": {"code": 487, "subCode": 0, "phrase": "CallerCancelled"}}),
            );
        }
    }

    /// Lets the latest joiner in.
    pub fn admit(&self) {
        if let Some(url) = self.link("/conversationRequest/links/localParticipantUpdate") {
            self.callback(
                &url,
                json!({"sequenceNumber": self.next_sequence(), "selfParticipant": {"state": "Connected"}}),
            );
        }
    }

    /// Someone joins or leaves (`None`) the meeting.
    pub fn roster(&self, id: &str, name: Option<&str>) {
        let entry = name
            .map(|n| json!({"displayName": n}))
            .unwrap_or(Value::Null);
        self.roster_entry(id, entry);
    }

    /// A raw roster entry for `id` (`null`: gone).
    pub fn roster_entry(&self, id: &str, entry: Value) {
        if let Some(url) = self.link("/conversationRequest/roster/rosterUpdate") {
            self.callback(
                &url,
                json!({"sequenceNumber": self.next_sequence(), "participants": {id: entry}}),
            );
        }
    }

    /// The recorder bot's roster entry, recording (`by` started it) or not.
    pub fn recorder(&self, call: &str, state: &str, by: Option<&str>) {
        let command_url = self.0.base.join(&format!("c/{call}/record")).unwrap();
        self.roster_entry(
            crate::payload::recording::BOT_MRIS[0],
            json!({
                "displayName": "Recorder",
                "endpoints": {"bot": {
                    "call": {},
                    "endpointMetadata": {
                        "commandUrl": command_url.as_str(),
                        "processingModes": {"recording": {"state": state, "userId": by}},
                    },
                }},
            }),
        );
    }

    /// The organizer ends the meeting for everyone.
    pub fn end(&self) {
        if let Some(url) = self.link("/conversationRequest/links/conversationEnd") {
            self.callback(
                &url,
                json!({"conversationEnd": {"code": 0, "subCode": 0, "phrase": "MeetingEnded"}}),
            );
        }
    }

    /// Answers the latest join with a 409 redirect to another conversation URL.
    pub fn redirect(&self) {
        if let Some(url) = self.link("/conversationRequest/links/conversationEnd") {
            let location = self.0.base.join("redirected/").unwrap();
            self.callback(
                &url,
                json!({"code": 409, "subCode": 5704, "conversationUrl": {"Location": location.as_str()}}),
            );
        }
    }
}

async fn join(
    State(fake): State<FakeConversation>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Json<Value> {
    let headers = headers
        .iter()
        .filter_map(|(k, v)| Some((k.as_str().to_owned(), v.to_str().ok()?.to_owned())))
        .collect();
    {
        let mut seen = fake.0.seen.lock().unwrap();
        seen.joins.push(body.clone());
        seen.join_headers.push(headers);
    }
    *fake.0.links.lock().unwrap() = body.clone();
    let id = fake.seen().joins.len();
    let controller = fake.0.base.join(&format!("c/{id}/")).unwrap();
    let link = |op: &str| controller.join(op).unwrap().to_string();
    let response = json!({
        "conversationController": controller.as_str(),
        "sequenceNumber": 1,
        "links": {
            "leave": link("leave"), "mute": link("mute"), "unmute": link("unmute"),
            "notificationLinks": link("links"), "admit": link("admit"), "addParticipant": link("add"),
        },
        "selfParticipant": {"enableCaptcha": false, "enforceConsentToJoin": false},
    });
    // Callbacks follow the synchronous answer, as on the real service.
    let pushed = fake.clone();
    let me = body
        .pointer("/participants/from/id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        pushed.roster("8:orgid:organizer", Some("Organisatrice"));
        pushed.roster(&me, Some("Moi"));
        // The final answer arrives inside acceptance. A separate mediaAnswer is optional.
        if let Some(blob) = pushed.media_blob() {
            if let Some(url) = pushed.link("/callInvitation/links/progress") {
                pushed.callback(&url, json!({"callProgress": {"code": 180}}));
            }
            tokio::time::sleep(std::time::Duration::from_millis(400)).await;
            if let Some(url) = pushed.link("/callInvitation/links/acceptance") {
                let acknowledgement = pushed.0.base.join("incoming/acknowledgement").unwrap();
                pushed.callback(
                    &url,
                    json!({"callAcceptance": {
                        "mediaContent": {"contentType": "application/sdp-ngc-1.0", "blob": blob},
                        "links": {"acknowledgement": acknowledgement.as_str()}
                    }}),
                );
            }
        }
        if pushed.0.lobby
            && let Some(url) = pushed.link("/conversationRequest/links/localParticipantUpdate")
        {
            pushed.callback(
                &url,
                json!({"sequenceNumber": pushed.next_sequence(), "selfParticipant": {"state": "InLobby"}}),
            );
        }
    });
    Json(response)
}

async fn operation(
    State(fake): State<FakeConversation>,
    Path((id, op)): Path<(String, String)>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> (StatusCode, Json<Value>) {
    let mut seen = fake.0.seen.lock().unwrap();
    match op.as_str() {
        "record" => {
            let authorization = headers
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned);
            seen.recorder_commands.push((body.clone(), authorization));
            drop(seen);
            let start = body.get("action").and_then(Value::as_str) == Some("start");
            let by = body.get("participantMri").and_then(Value::as_str);
            fake.recorder(&id, if start { "Active" } else { "Inactive" }, by);
            return (StatusCode::OK, Json(json!({})));
        }
        "leave" => seen.leaves.push(body),
        "mute" | "unmute" => seen.mutes.push((op, body)),
        "links" => seen.keep_alives += 1,
        "add" => {
            seen.added.extend(
                body.pointer("/participants/to")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|p| p.get("id").and_then(Value::as_str).map(str::to_owned)),
            );
            let added = seen.added.clone();
            drop(seen);
            for added in added {
                if crate::payload::recording::is_bot(&added) {
                    fake.recorder(&id, "Inactive", None);
                } else {
                    fake.roster(&added, Some("Echo Bot"));
                }
            }
            return (StatusCode::OK, Json(json!({})));
        }
        _ => return (StatusCode::NOT_FOUND, Json(Value::Null)),
    }
    (StatusCode::OK, Json(json!({})))
}

async fn incoming(
    State(fake): State<FakeConversation>,
    Path(op): Path<String>,
    Json(body): Json<Value>,
) -> (StatusCode, Json<Value>) {
    let link = |op: &str| fake.incoming_link(op);
    match op.as_str() {
        "attach" => {
            let offer = {
                let mut attach = fake.0.attach.lock().unwrap();
                let offer = attach["offer"].clone();
                *attach = json!({"offer": offer, "attach": body["attach"].clone()});
                offer
            };
            fake.0.seen.lock().unwrap().attaches.push(body);
            (
                StatusCode::OK,
                Json(json!({
                    "callInvitation": {
                        "mediaContent": {"contentType": "application/sdp-ngc-1.0", "blob": offer},
                        "links": {"progress": link("progress"), "acceptance": link("accept"), "callLeg": link("reject")},
                    },
                    "links": {"leave": fake.0.base.join("c/incoming/leave").unwrap().as_str()},
                })),
            )
        }
        "accept" => {
            let callbacks = body
                .pointer("/callAcceptance/links/mediaRenegotiation")
                .and_then(Value::as_str)
                .and_then(|u| u.strip_suffix("/call/mediaRenegotiation/"))
                .map(str::to_owned);
            fake.0.seen.lock().unwrap().accepts.push(body);
            if let Some(base) = callbacks {
                let pushed = fake.clone();
                tokio::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
                    pushed.callback(
                        &format!("{base}/call/acknowledgement/"),
                        json!({"callAcceptanceAcknowledgement": {"links": {}}}),
                    );
                });
            }
            (StatusCode::OK, Json(json!({})))
        }
        "progress" => {
            fake.0.seen.lock().unwrap().ringing += 1;
            (StatusCode::OK, Json(json!({})))
        }
        "acknowledgement" => {
            fake.0.seen.lock().unwrap().acknowledgements += 1;
            (StatusCode::OK, Json(json!({})))
        }
        _ => (StatusCode::NOT_FOUND, Json(Value::Null)),
    }
}

async fn reject(State(fake): State<FakeConversation>, Path(op): Path<String>) -> StatusCode {
    if op != "reject" {
        return StatusCode::NOT_FOUND;
    }
    fake.0.seen.lock().unwrap().rejects += 1;
    StatusCode::OK
}

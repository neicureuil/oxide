use super::*;
use crate::task::Job;
use serde_json::json;

fn ready() -> App {
    let mut app = App::empty();
    *app.client = Some(ChatClient::new(Arc::new(AppAuthenticator::Demo(
        StaticAuthenticator::new().with_service(
            Service::CallingConversation,
            Url::parse("https://conv.example/epconv/").unwrap(),
        ),
    ))));
    *app.me = "8:orgid:me".into();
    app.calls.surl = Some("https://t.example/s".into());
    app.calls.session = "sess".into();
    app.calls.push.endpoint = "ep".into();
    app
}

fn effects(task: &Task<Msg>) -> Vec<Effect> {
    task.effects().cloned().collect()
}

/// Runs a task like the core: futures and subscriptions send to `tx`, effects are returned.
fn run(task: Task<Msg>, tx: &tokio::sync::mpsc::UnboundedSender<Msg>) -> Vec<Effect> {
    let mut effects = Vec::new();
    for job in task.0 {
        match job {
            Job::Effect(effect) => effects.push(effect),
            Job::Future(future) => {
                let tx = tx.clone();
                tokio::spawn(async move {
                    let _ = tx.send(future.await);
                });
            }
            Job::Subscribe(start) => {
                let tx = tx.clone();
                tokio::spawn(start(crate::task::Sink::new(move |msg| {
                    let _ = tx.send(msg);
                })));
            }
            Job::Done(msg) => {
                let _ = tx.send(msg);
            }
        }
    }
    effects
}

#[tokio::test]
async fn a_full_call_runs_over_the_fake_push_channel_and_conversation_service() {
    let trouter = teams_realtime::fake::FakeTrouter::start().await;
    let conversation = teams_calling::fake::FakeConversation::start(trouter.clone(), false).await;
    let mut app = App::empty();
    *app.client = Some(ChatClient::new(Arc::new(AppAuthenticator::Demo(
        StaticAuthenticator::new()
            .with_service(Service::Trouter, trouter.trouter_url())
            .with_service(Service::Registrar, trouter.registrar_url())
            .with_service(Service::CallingConversation, conversation.url()),
    ))));
    *app.me = "8:orgid:me".into();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let mut effects = run(app.sync_realtime(), &tx);
    let mut started = false;
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            // Play the native media engine.
            for effect in std::mem::take(&mut effects) {
                let action = match effect {
                    Effect::MediaPrepare { call_id, remote_offer: None, .. } => {
                        CallAction::LocalDescription {
                            call_id,
                            sdp: "v=0\r\nm=audio 9 UDP/TLS/RTP/SAVPF 111\r\na=mid:0\r\na=sendrecv\r\na=rtpmap:111 opus/48000/2\r\na=ssrc:42 cname:x\r\n".into(),
                        }
                    }
                    Effect::MediaRemoteDescription { call_id, sdp } => {
                        assert!(sdp.contains("UDP/TLS/RTP/SAVPF") || sdp.contains("RTP/SAVPF"));
                        CallAction::MediaConnected { call_id }
                    }
                    _ => continue,
                };
                effects.extend(run(app.update_call(CallMsg::Action(action)), &tx));
            }
            if !started && app.calls.view.realtime == "Prêt à recevoir les appels" {
                started = true;
                effects.extend(run(
                    app.update_call(CallMsg::Action(CallAction::Start { mri: "8:orgid:bob".into(), video: false })),
                    &tx,
                ));
            }
            if app.call_part().active.is_some_and(|c| c.status == "En communication") {
                return;
            }
            let msg = rx.recv().await.expect("core stopped");
            effects.extend(run(app.update(msg), &tx));
        }
    })
    .await
    .expect("the call never connected");

    let start = &conversation.seen().joins[0];
    let blob = start["callInvitation"]["mediaContent"]["blob"]
        .as_str()
        .unwrap();
    assert!(
        blob.contains("m=audio 9 RTP/SAVP 111"),
        "NGC offer sent: {blob}"
    );
    assert!(blob.contains("a=label:main-audio"));
    assert_eq!(start["participants"]["to"][0]["id"], "8:orgid:bob");

    effects.extend(run(
        app.update_call(CallMsg::Action(CallAction::Hangup)),
        &tx,
    ));
    tokio::time::timeout(Duration::from_secs(5), async {
        while app
            .call_part()
            .active
            .is_some_and(|c| c.phase != CallPhaseView::Ended)
        {
            let msg = rx.recv().await.unwrap();
            effects.extend(run(app.update(msg), &tx));
        }
    })
    .await
    .expect("hang up never completed");
    assert_eq!(conversation.seen().leaves.len(), 1);
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::MediaClose { .. }))
    );
}

#[test]
fn meeting_chats_open_the_prejoin_screen_and_join_with_the_chosen_devices() {
    let mut app = ready();
    let chat: Conversation = serde_json::from_value(json!({
        "id": "19:meeting_abc@thread.v2",
        "meetingInformation": {"subject": "Revue", "organizerId": "org", "tenantId": "ten"}
    }))
    .unwrap();
    app.chats.push(chat);
    *app.selected = Some(Selection::Chat("19:meeting_abc@thread.v2".into()));
    assert!(app.conversation_part().view.unwrap().is_meeting);

    let _ = app.update_call(CallMsg::Action(CallAction::PrepareChatMeeting));
    assert_eq!(app.call_part().prejoin.unwrap().title, "Revue");
    let Some(CallKind::Meeting(meeting)) = app.call.prejoin.as_ref().map(|p| &p.kind) else {
        panic!()
    };
    assert_eq!(meeting.thread_id, "19:meeting_abc@thread.v2");
    assert_eq!(meeting.organizer_id.as_deref(), Some("org"));
    assert_eq!(meeting.tenant_id.as_deref(), Some("ten"));

    let _ = app.update_call(CallMsg::Action(CallAction::CancelJoin));
    assert!(app.call_part().prejoin.is_none());

    let _ = app.update_call(CallMsg::Action(CallAction::PrepareChatMeeting));
    let _ = app.update_call(CallMsg::Action(CallAction::ConfirmJoin {
        video: true,
        muted: true,
        speaker: true,
        transfer: false,
    }));
    let part = app.call_part();
    assert!(part.prejoin.is_none());
    let call = part.active.unwrap();
    assert!(call.is_meeting && call.muted && call.camera);
    let task = app.update_call(CallMsg::Relays(app.generation, call.id.clone(), vec![]));
    assert!(matches!(
        &effects(&task)[..],
        [Effect::MediaPrepare {
            video: true,
            muted: true,
            ..
        }]
    ));
    let task = app.update_call(CallMsg::Action(CallAction::SetCamera { on: false }));
    assert_eq!(
        effects(&task),
        [Effect::MediaSetCamera {
            call_id: call.id,
            on: false
        }]
    );
    assert!(!app.call_part().active.unwrap().camera);
}

#[test]
fn regular_chats_and_calendar_links_without_coordinates_are_refused() {
    let mut app = ready();
    app.chats
        .push(serde_json::from_value(json!({"id": "19:abc@thread.v2"})).unwrap());
    *app.selected = Some(Selection::Chat("19:abc@thread.v2".into()));
    assert!(!app.conversation_part().view.unwrap().is_meeting);
    let _ = app.update_call(CallMsg::Action(CallAction::PrepareChatMeeting));
    assert!(app.call_part().prejoin.is_none());
    assert!(app.call_part().error.is_some());
    let _ = app.update_call(CallMsg::Action(CallAction::PrepareMeeting {
        url: "https://teams.microsoft.com/v2/#/calendarv2".into(),
        title: "x".into(),
    }));
    assert!(app.call_part().prejoin.is_none());
}

#[test]
fn roster_roles_consent_and_connection_time_reach_the_view() {
    let mut app = ready();
    let _ = app.update_call(CallMsg::Action(CallAction::PrepareMeeting {
        url: "https://teams.microsoft.com/l/meetup-join/19%3ameeting_x%40thread.v2/0?context=%7b%22Tid%22%3a%22t%22%7d".into(),
        title: "Point".into(),
    }));
    let _ = app.update_call(CallMsg::Action(CallAction::ConfirmJoin {
        video: false,
        muted: false,
        speaker: true,
        transfer: false,
    }));
    let id = app.call_part().active.unwrap().id;
    let _ = app.update_call(CallMsg::Action(CallAction::LocalDescription {
        call_id: id.clone(),
        sdp: "v=0".into(),
    }));
    let _ = app.update_call(CallMsg::Http(
        app.generation,
        id.clone(),
        Op::Join,
        Ok(json!({"links": {"leave": "https://conv.example/l"},
            "selfParticipant": {"enforceConsentToJoin": true}})),
    ));
    let pushed = |name: &str, body: serde_json::Value| teams_realtime::PushedRequest {
        method: "POST".into(),
        url: String::new(),
        path: format!("/callAgent/sess/{id}/{name}"),
        headers: Default::default(),
        body: body.to_string(),
    };
    let _ = app.call_pushed(&pushed(
        "conversation/rosterUpdate/",
        json!({"participants": {
        "8:orgid:me": {"displayName": "Moi"},
        "8:orgid:o": {"displayName": "Olga", "role": "Organizer"},
        "8:orgid:g": {"displayName": "Invité", "state": "InLobby"}}}),
    ));
    let _ = app.call_pushed(&pushed("call/acceptance/", json!({})));
    let call = app.call_part().active.unwrap();
    assert_eq!(call.title, "Point");
    assert!(call.notice.is_some());
    assert!(call.connected_at_ms.is_some());
    assert_eq!(call.participants.len(), 2, "without oneself");
    let olga = call.participants.iter().find(|p| p.name == "Olga").unwrap();
    assert_eq!(olga.role, "organizer");
    assert!(
        call.participants
            .iter()
            .any(|p| p.name == "Invité" && p.in_lobby)
    );
}

fn chat(id: &str, members: &[&str]) -> Conversation {
    serde_json::from_value(json!({
        "id": id,
        "members": members.iter().map(|m| json!({"id": m})).collect::<Vec<_>>(),
    }))
    .unwrap()
}

#[test]
fn chats_call_every_other_member_and_tie_group_calls_to_the_chat() {
    let mut app = ready();
    app.chats.push(chat(
        "19:a_b@unq.gbl.spaces",
        &["8:orgid:me", "8:orgid:bob"],
    ));
    app.chats.push(chat(
        "19:group@thread.v2",
        &[
            "8:orgid:me",
            "8:orgid:a",
            "8:orgid:b",
            "8:orgid:A",
            "28:bot",
        ],
    ));
    *app.selected = Some(Selection::Chat("19:a_b@unq.gbl.spaces".into()));
    assert!(app.conversation_part().view.unwrap().can_call);

    let _ = app.update_call(CallMsg::Action(CallAction::CallChat {
        chat_id: None,
        video: true,
    }));
    let Some(call) = &app.call.active else {
        panic!("{:?}", app.call.error)
    };
    let CallKind::Outgoing { to, chat_thread } = &call.kind else {
        panic!()
    };
    assert_eq!(
        to.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
        ["8:orgid:bob"]
    );
    assert_eq!(chat_thread.as_deref(), Some("19:a_b@unq.gbl.spaces"));
    assert!(call.video);

    let (kind, _) = app.chat_call("19:group@thread.v2").unwrap();
    let CallKind::Outgoing { to, chat_thread } = kind else {
        panic!()
    };
    assert_eq!(to.len(), 2, "self, duplicates and bots are skipped");
    assert_eq!(chat_thread.as_deref(), Some("19:group@thread.v2"));

    let many: Vec<String> = (0..=MAX_GROUP_CALL)
        .map(|i| format!("8:orgid:{i}"))
        .collect();
    let refs: Vec<&str> = many.iter().map(String::as_str).collect();
    app.chats.push(chat("19:big@thread.v2", &refs));
    assert!(
        app.chat_call("19:big@thread.v2")
            .unwrap_err()
            .contains("réunion")
    );
    app.chats.push(chat("48:notes", &["8:orgid:me"]));
    assert!(app.chat_call("48:notes").is_err());
}

#[test]
fn history_calls_back_the_person_or_the_group_chat() {
    let mut app = ready();
    app.chats.push(chat(
        "19:group@thread.v2",
        &["8:orgid:me", "8:orgid:a", "8:orgid:b"],
    ));
    let log = |id: &str, extra: serde_json::Value| {
        let mut raw = json!({"callId": id, "callDirection": "incoming", "callState": "missed",
            "originatorParticipant": {"id": "8:orgid:bob"}});
        raw.as_object_mut()
            .unwrap()
            .extend(extra.as_object().cloned().unwrap());
        let message: Message =
            serde_json::from_value(json!({"id": id, "properties": {"call-log": raw.to_string()}}))
                .unwrap();
        teams_calling::CallLog::from_message(crate::app::calls::log_message(&message)).unwrap()
    };
    app.calls_logs_for_test(vec![
        log("single", json!({})),
        log(
            "group",
            json!({"callType": "MultiParty", "groupChatThreadId": "19:group@thread.v2"}),
        ),
    ]);
    let _ = app.update_calls(calls::CallsMsg::Action(calls::CallsAction::CallBack {
        id: "single".into(),
        video: false,
    }));
    let Some(call) = &app.call.active else {
        panic!()
    };
    assert!(
        matches!(&call.kind, CallKind::Outgoing { to, chat_thread: None } if to[0].id == "8:orgid:bob")
    );

    app.call.active = None;
    let _ = app.update_calls(calls::CallsMsg::Action(calls::CallsAction::CallBack {
        id: "group".into(),
        video: false,
    }));
    let Some(call) = &app.call.active else {
        panic!()
    };
    assert!(matches!(&call.kind, CallKind::Outgoing { to, chat_thread: Some(_) } if to.len() == 2));
}

/// Native calls on, push channel registered, against the fakes.
async fn connected() -> (
    App,
    teams_calling::fake::FakeConversation,
    tokio::sync::mpsc::UnboundedSender<Msg>,
    tokio::sync::mpsc::UnboundedReceiver<Msg>,
) {
    let trouter = teams_realtime::fake::FakeTrouter::start().await;
    let conversation = teams_calling::fake::FakeConversation::start(trouter.clone(), false).await;
    let mut app = App::empty();
    *app.client = Some(ChatClient::new(Arc::new(AppAuthenticator::Demo(
        StaticAuthenticator::new()
            .with_service(Service::Trouter, trouter.trouter_url())
            .with_service(Service::Registrar, trouter.registrar_url())
            .with_service(Service::CallingConversation, conversation.url()),
    ))));
    *app.me = "8:orgid:me".into();
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    run(app.sync_realtime(), &tx);
    tokio::time::timeout(Duration::from_secs(5), async {
        while app.calls.view.realtime != "Prêt à recevoir les appels" {
            let msg = rx.recv().await.unwrap();
            run(app.update(msg), &tx);
        }
    })
    .await
    .expect("push channel not ready");
    (app, conversation, tx, rx)
}

/// Feeds messages and plays the media engine until `done`.
async fn until(
    app: &mut App,
    tx: &tokio::sync::mpsc::UnboundedSender<Msg>,
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<Msg>,
    done: impl Fn(&CallPart) -> bool,
) -> Vec<Effect> {
    let mut seen = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), async {
        while !done(&app.call_part()) {
            let msg = rx.recv().await.unwrap();
            for effect in run(app.update(msg), tx) {
                if let Effect::MediaPrepare { call_id, remote_offer: Some(offer), .. } = &effect {
                    assert!(offer.contains("UDP/TLS/RTP/SAVPF"), "offer converted for WebRTC");
                    let answer = "v=0\r\nm=audio 9 UDP/TLS/RTP/SAVPF 111\r\na=mid:0\r\na=rtpmap:111 opus/48000/2\r\n";
                    for effect in run(
                        app.update_call(CallMsg::Action(CallAction::LocalDescription {
                            call_id: call_id.clone(),
                            sdp: answer.into(),
                        })),
                        tx,
                    ) {
                        seen.push(effect);
                    }
                    run(app.update_call(CallMsg::Action(CallAction::MediaConnected { call_id: call_id.clone() })), tx);
                }
                seen.push(effect);
            }
        }
    })
    .await
    .expect("state never reached");
    seen
}

#[tokio::test]
async fn incoming_calls_ring_and_can_be_answered() {
    let (mut app, conversation, tx, mut rx) = connected().await;
    conversation.ring_me("8:orgid:alice", "Alice Martin", true);
    until(&mut app, &tx, &mut rx, |part| {
        part.active.as_ref().is_some_and(|c| c.can_accept)
    })
    .await;
    let call = app.call_part().active.unwrap();
    assert_eq!(call.phase, CallPhaseView::Incoming);
    assert_eq!(call.title, "Alice Martin");
    assert!(call.offers_video);
    assert_eq!(conversation.seen().attaches.len(), 1);

    run(
        app.update_call(CallMsg::Action(CallAction::Accept { video: false })),
        &tx,
    );
    until(&mut app, &tx, &mut rx, |part| {
        part.active
            .as_ref()
            .is_some_and(|c| c.status == "En communication")
    })
    .await;
    let seen = conversation.seen();
    assert_eq!(
        seen.acknowledgements, 0,
        "the callee receives the acknowledgement"
    );
    assert_eq!(seen.ringing, 1);
    let accept = &seen.accepts[0]["callAcceptance"];
    assert_eq!(accept["acceptedCallModalities"], json!(["Audio"]));
    assert!(
        accept["mediaContent"]["blob"]
            .as_str()
            .unwrap()
            .contains("m=audio 9 RTP/SAVP 111")
    );
}

#[tokio::test]
async fn incoming_calls_can_be_declined_or_missed() {
    let (mut app, conversation, tx, mut rx) = connected().await;
    conversation.ring_me("8:orgid:alice", "Alice Martin", false);
    until(&mut app, &tx, &mut rx, |part| {
        part.active.as_ref().is_some_and(|c| c.can_accept)
    })
    .await;
    run(app.update_call(CallMsg::Action(CallAction::Decline)), &tx);
    let effects = until(&mut app, &tx, &mut rx, |part| {
        part.active
            .as_ref()
            .is_some_and(|c| c.phase == CallPhaseView::Ended)
    })
    .await;
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::MediaClose { .. }))
    );
    assert_eq!(app.call_part().active.unwrap().status, "Appel refusé.");
    assert_eq!(conversation.seen().rejects, 1);
    run(app.update_call(CallMsg::Action(CallAction::Dismiss)), &tx);

    conversation.ring_me("8:orgid:bob", "", false);
    until(&mut app, &tx, &mut rx, |part| {
        part.active.as_ref().is_some_and(|c| c.can_accept)
    })
    .await;
    assert_eq!(
        app.call_part().active.unwrap().title,
        "Correspondant inconnu"
    );
    // A second caller while this one rings is not picked up.
    conversation.ring_me("8:orgid:carol", "Carol", false);
    conversation.cancel_incoming();
    until(&mut app, &tx, &mut rx, |part| {
        part.active
            .as_ref()
            .is_some_and(|c| c.phase == CallPhaseView::Ended)
    })
    .await;
    assert_eq!(app.call_part().active.unwrap().status, "Appel manqué.");
    assert_eq!(conversation.seen().attaches.len(), 2);
}

#[test]
fn connected_calls_survive_renegotiation_network_changes_and_a_new_push_session() {
    let mut app = ready();
    let _ = app.update_call(CallMsg::Action(CallAction::Start {
        mri: "8:orgid:bob".into(),
        video: false,
    }));
    let id = app.call_part().active.unwrap().id;
    let _ = app.update_call(CallMsg::Action(CallAction::LocalDescription {
        call_id: id.clone(),
        sdp: "v=0".into(),
    }));
    let _ = app.update_call(CallMsg::Http(
        app.generation,
        id.clone(),
        Op::Start,
        Ok(json!({"links": {"leave": "https://conv.example/leave",
            "mediaRenegotiation": "https://conv.example/reneg",
            "notificationLinks": "https://conv.example/links"}})),
    ));
    let pushed = |name: &str, body: serde_json::Value| teams_realtime::PushedRequest {
        method: "POST".into(),
        url: String::new(),
        path: format!("/callAgent/sess/{id}/{name}"),
        headers: Default::default(),
        body: body.to_string(),
    };
    let _ = app.call_pushed(&pushed("call/acceptance/", json!({})));
    assert!(app.call_in_progress());

    // The service renegotiates.
    let task = app.call_pushed(&pushed("call/mediaRenegotiation/", json!({"mediaNegotiation": {
        "mediaContent": {"blob": "v=0\ns=offer"}, "links": {"mediaAnswer": "https://conv.example/answer"}}})));
    assert!(
        matches!(&effects(&task)[..], [Effect::MediaRemoteOffer { sdp, .. }] if sdp.contains("s=offer"))
    );
    let task = app.update_call(CallMsg::Action(CallAction::RenegotiationAnswer {
        call_id: id.clone(),
        sdp: "v=0".into(),
    }));
    assert_eq!(task.0.len(), 1, "answer sent");

    // The network changes: ICE restart, and the new local offer is renegotiated.
    let task = app.update_call(CallMsg::Action(CallAction::NetworkChanged));
    assert!(effects(&task).contains(&Effect::MediaRestartIce {
        call_id: id.clone()
    }));
    let task = app.update_call(CallMsg::Action(CallAction::LocalDescription {
        call_id: id.clone(),
        sdp: "v=0\ns=restart".into(),
    }));
    assert_eq!(task.0.len(), 1, "renegotiation sent");

    // The camera's renegotiation is rejected: the engine rolls back and offers again.
    let task = app.update_call(CallMsg::Action(CallAction::SetCamera { on: true }));
    assert!(effects(&task).contains(&Effect::MediaSetCamera {
        call_id: id.clone(),
        on: true
    }));
    let _ = app.update_call(CallMsg::Action(CallAction::LocalDescription {
        call_id: id.clone(),
        sdp: "v=0\ns=camera".into(),
    }));
    let task = app.call_pushed(&pushed(
        "call/mediaRejection/r2",
        json!({"mediaNegotiationFailure": {}, "debugContent": {}}),
    ));
    assert_eq!(
        effects(&task),
        [Effect::MediaOfferRejected {
            call_id: id.clone(),
            retry: true
        }]
    );
    // Out of retries, the engine stops the camera.
    let _ = app.update_call(CallMsg::Action(CallAction::CameraStopped {
        call_id: id.clone(),
        reason: Some("refused".into()),
    }));
    assert!(!app.call.active.as_ref().unwrap().camera);
    assert_eq!(app.call.error.as_deref(), Some("refused"));
    assert_eq!(
        app.call_part().active.unwrap().phase,
        CallPhaseView::Connected
    );

    // The push channel comes back with another session URL: callbacks follow.
    app.calls.push.stop = Some(tokio::sync::watch::channel(false).0);
    let task = app.update_calls(calls::CallsMsg::Realtime(
        app.generation,
        teams_realtime::RealtimeEvent::Connected {
            surl: "https://t2.example/s2".into(),
        },
    ));
    assert_eq!(task.0.len(), 1, "notification links renewed");
    let task = app.update_calls(calls::CallsMsg::Realtime(
        app.generation,
        teams_realtime::RealtimeEvent::Connected {
            surl: "https://t2.example/s2".into(),
        },
    ));
    assert!(task.0.is_empty(), "same URL: nothing to do");
}

#[tokio::test]
async fn the_test_call_adds_the_echo_bot_and_connects() {
    let (mut app, conversation, tx, mut rx) = connected().await;
    app.chats.push(chat(
        "19:bot@unq.gbl.spaces",
        &["8:orgid:me", &echo_bot_mri()],
    ));
    run(
        app.update_call(CallMsg::Action(CallAction::TestCall { bot_mri: None })),
        &tx,
    );
    let call = app.call_part().active.unwrap();
    assert!(call.is_test && call.video);
    assert_eq!(call.title, "Appel de test");
    // Play the media engine for the offer.
    let start = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(start) = conversation.seen().joins.first().cloned() {
                return start;
            }
            let msg = rx.recv().await.unwrap();
            for effect in run(app.update(msg), &tx) {
                if let Effect::MediaPrepare { call_id, remote_offer: None, video, .. } = effect {
                    assert!(video, "the test call asks for video");
                    run(
                        app.update_call(CallMsg::Action(CallAction::LocalDescription {
                            call_id,
                            sdp: "v=0\r\nm=audio 9 UDP/TLS/RTP/SAVPF 111\r\na=mid:0\r\na=rtpmap:111 opus/48000/2\r\n".into(),
                        })),
                        &tx,
                    );
                }
            }
        }
    })
    .await
    .expect("the test call never started");
    assert!(start["participants"].get("to").is_none());
    assert_eq!(start["groupChat"]["threadId"], "19:bot@unq.gbl.spaces");
    until(&mut app, &tx, &mut rx, |part| {
        part.active
            .as_ref()
            .is_some_and(|c| c.participants.iter().any(|p| p.id == echo_bot_mri()))
    })
    .await;
    assert_eq!(conversation.seen().added, [echo_bot_mri()]);
    assert_eq!(
        app.call_part().active.unwrap().phase,
        CallPhaseView::Connected
    );
}

#[test]
fn without_a_bot_chat_the_test_call_rings_the_configured_bot() {
    let mut app = ready();
    let _ = app.update_call(CallMsg::Action(CallAction::TestCall {
        bot_mri: Some(" 28:other ".into()),
    }));
    let Some(call) = &app.call.active else {
        panic!()
    };
    let CallKind::EchoTest { bot, chat_thread } = &call.kind else {
        panic!()
    };
    assert_eq!(bot.id, "28:other");
    assert!(chat_thread.is_none());
    app.call.active = None;
    let _ = app.update_call(CallMsg::Action(CallAction::TestCall {
        bot_mri: Some("8:orgid:x".into()),
    }));
    let Some(call) = &app.call.active else {
        panic!()
    };
    assert!(
        matches!(&call.kind, CallKind::EchoTest { bot, .. } if bot.id == echo_bot_mri()),
        "only bots"
    );
}

#[test]
fn echo_conversation_resolution_is_cached_and_late_results_do_not_restart_calls() {
    let mut app = ready();
    let _ = app.update_call(CallMsg::Action(CallAction::TestCall { bot_mri: None }));
    let id = app.call_part().active.unwrap().id;
    let task = app.update_call(CallMsg::EchoChat(
        app.generation,
        id.clone(),
        Ok("19:echo".into()),
    ));
    assert_eq!(
        task.0.len(),
        1,
        "TURN preparation follows conversation resolution"
    );
    assert!(matches!(&app.call.active.as_ref().unwrap().kind,
        CallKind::EchoTest { chat_thread: Some(thread), .. } if thread == "19:echo"));
    let _ = app.update_call(CallMsg::Action(CallAction::Hangup));
    let task = app.update_call(CallMsg::EchoChat(app.generation, id, Ok("19:late".into())));
    assert!(task.0.is_empty());
    let _ = app.update_call(CallMsg::Action(CallAction::TestCall { bot_mri: None }));
    assert!(matches!(&app.call.active.as_ref().unwrap().kind,
        CallKind::EchoTest { chat_thread: Some(thread), .. } if thread == "19:echo"));
}

#[test]
fn a_call_needs_the_push_channel() {
    let mut app = ready();
    app.calls.surl = None;
    let task = app.update_call(CallMsg::Action(CallAction::Start {
        mri: "8:orgid:bob".into(),
        video: false,
    }));
    assert!(task.0.is_empty());
    assert!(app.call_part().error.unwrap().contains("pas encore prêt"));
}

#[test]
fn outgoing_call_prepares_media_then_starts_and_hands_the_answer_to_media() {
    let mut app = ready();
    let task = app.update_call(CallMsg::Action(CallAction::Start {
        mri: "8:orgid:bob".into(),
        video: false,
    }));
    assert!(
        matches!(task.0.as_slice(), [Job::Future(_)]),
        "relays are fetched first"
    );
    let id = app.call_part().active.unwrap().id;
    assert_eq!(
        app.call_part().active.unwrap().phase,
        CallPhaseView::Preparing
    );

    let task = app.update_call(CallMsg::Relays(app.generation, id.clone(), vec![]));
    assert!(matches!(
        &effects(&task)[..],
        [Effect::MediaPrepare {
            remote_offer: None,
            video: false,
            ..
        }]
    ));

    // A second call is refused while this one runs.
    let _ = app.update_call(CallMsg::Action(CallAction::Start {
        mri: "8:orgid:c".into(),
        video: false,
    }));
    assert!(app.call_part().error.is_some());

    let task = app.update_call(CallMsg::Action(CallAction::LocalDescription {
        call_id: id.clone(),
        sdp: "v=0\no=- 1 1 IN IP4 0.0.0.0\n".into(),
    }));
    assert_eq!(task.0.len(), 2, "start request and establish timer");
    assert_eq!(
        app.call_part().active.unwrap().phase,
        CallPhaseView::Connecting
    );

    let _ = app.update_call(CallMsg::Http(
        app.generation,
        id.clone(),
        Op::Start,
        Ok(json!({"links": {"leave": "https://conv.example/leave"}})),
    ));
    let request = teams_realtime::PushedRequest {
        method: "POST".into(),
        url: String::new(),
        path: format!("/callAgent/sess/{id}/call/mediaAnswer/"),
        headers: Default::default(),
        body: json!({"mediaAnswer": {"mediaContent": {"blob": "v=0\ns=answer"}}}).to_string(),
    };
    let task = app.call_pushed(&request);
    assert_eq!(
        effects(&task),
        [Effect::MediaRemoteProvisionalDescription {
            call_id: id.clone(),
            sdp: "v=0\r\ns=answer\r\na=extmap-allow-mixed\r\n".into()
        }]
    );
    let accepted = teams_realtime::PushedRequest {
        path: format!("/callAgent/sess/{id}/call/acceptance/"),
        body: "{}".into(),
        ..request
    };
    let _ = app.call_pushed(&accepted);
    let _ = app.update_call(CallMsg::Action(CallAction::MediaConnected {
        call_id: id.clone(),
    }));
    assert_eq!(app.call_part().active.unwrap().status, "En communication");

    let task = app.update_call(CallMsg::Action(CallAction::SetMuted { muted: true }));
    assert!(effects(&task).contains(&Effect::MediaSetMuted {
        call_id: id.clone(),
        muted: true
    }));

    let task = app.update_call(CallMsg::Action(CallAction::Hangup));
    assert_eq!(task.0.len(), 1, "leave request");
    let task = app.update_call(CallMsg::Http(
        app.generation,
        id.clone(),
        Op::Leave,
        Ok(Value::Null),
    ));
    assert_eq!(
        effects(&task),
        [Effect::MediaClose {
            call_id: id.clone()
        }]
    );
    assert_eq!(app.call_part().active.unwrap().phase, CallPhaseView::Ended);
    let _ = app.update_call(CallMsg::Action(CallAction::Dismiss));
    assert!(app.call_part().active.is_none());
}

#[test]
fn hanging_up_before_media_is_ready_only_closes_media() {
    let mut app = ready();
    let _ = app.update_call(CallMsg::Action(CallAction::JoinMeeting {
        url: "https://teams.microsoft.com/l/meetup-join/19%3ameeting_x%40thread.v2/0?context=%7b%22Tid%22%3a%22t%22%7d".into(),
        video: true,
    }));
    let id = app.call_part().active.unwrap().id;
    let task = app.update_call(CallMsg::Action(CallAction::Hangup));
    assert_eq!(
        effects(&task),
        [Effect::MediaClose {
            call_id: id.clone()
        }]
    );
    // The late local description is ignored.
    let task = app.update_call(CallMsg::Action(CallAction::LocalDescription {
        call_id: id,
        sdp: "v=0".into(),
    }));
    assert!(task.0.is_empty());
}

#[test]
fn the_prejoin_screen_spots_our_other_devices_and_can_move_the_meeting_here() {
    let mut app = ready();
    let task = app.update_call(CallMsg::Action(CallAction::PrepareMeeting {
        url: "https://teams.microsoft.com/l/meetup-join/19%3ameeting_x%40thread.v2/0?context=%7b%22Tid%22%3a%22t%22%7d".into(),
        title: "Point".into(),
    }));
    assert_eq!(task.0.len(), 1, "roster-only join");
    let watch = app
        .call
        .prejoin
        .as_ref()
        .unwrap()
        .watch
        .as_ref()
        .unwrap()
        .id
        .clone();
    let _ = app.update_call(CallMsg::Http(
        app.generation,
        watch.clone(),
        Op::Join,
        Ok(json!({"links": {"leave": "https://conv.example/watch/leave"}})),
    ));
    let _ = app.call_pushed(&teams_realtime::PushedRequest {
        method: "POST".into(),
        url: String::new(),
        path: format!("/callAgent/sess/{watch}/conversation/rosterUpdate/"),
        headers: Default::default(),
        body: json!({"sequenceNumber": 3, "participants": {"8:orgid:me": {
            "state": "active", "details": {"id": "8:orgid:me"},
            "endpoints": {"phone": {"call": {}}},
        }}})
        .to_string(),
    });
    assert_eq!(app.call_part().prejoin.unwrap().other_devices, 1);

    let task = app.update_call(CallMsg::Action(CallAction::ConfirmJoin {
        video: false,
        muted: true,
        speaker: true,
        transfer: true,
    }));
    assert_eq!(
        task.0.len(),
        3,
        "watch left, relays fetched, meeting chat unmuted"
    );
    let call = app.call.active.as_ref().unwrap();
    assert!(call.replace_other_devices && call.muted);
    let id = call.id.clone();
    let _ = app.update_call(CallMsg::Action(CallAction::LocalDescription {
        call_id: id,
        sdp: "v=0".into(),
    }));
    let session = app.call.active.as_ref().unwrap().session.as_ref().unwrap();
    assert!(session.replace_other_devices);
}

#[test]
fn an_added_device_can_turn_its_speaker_back_on() {
    let mut app = ready();
    let _ = app.update_call(CallMsg::Action(CallAction::PrepareMeeting {
        url: "https://teams.microsoft.com/l/meetup-join/19%3ameeting_x%40thread.v2/0?context=%7b%22Tid%22%3a%22t%22%7d".into(),
        title: "Point".into(),
    }));
    let _ = app.update_call(CallMsg::Action(CallAction::ConfirmJoin {
        video: false,
        muted: true,
        speaker: false,
        transfer: false,
    }));
    let id = app.call_part().active.unwrap().id;
    let task = app.update_call(CallMsg::Relays(app.generation, id.clone(), vec![]));
    assert!(matches!(
        &effects(&task)[..],
        [Effect::MediaPrepare {
            speaker: false,
            muted: true,
            ..
        }]
    ));
    let task = app.update_call(CallMsg::Action(CallAction::SetSpeaker { on: true }));
    assert_eq!(
        effects(&task),
        [Effect::MediaSetSpeaker {
            call_id: id,
            on: true
        }]
    );
    assert!(app.call_part().active.unwrap().speaker);
}

#[test]
fn cloud_recording_shows_who_records_and_hides_the_recorder_bot() {
    let mut app = ready();
    let _ = app.update_call(CallMsg::Action(CallAction::PrepareMeeting {
        url: "https://teams.microsoft.com/l/meetup-join/19%3ameeting_x%40thread.v2/0?context=%7b%22Tid%22%3a%22t%22%7d".into(),
        title: "Point".into(),
    }));
    let _ = app.update_call(CallMsg::Action(CallAction::ConfirmJoin {
        video: false,
        muted: false,
        speaker: true,
        transfer: false,
    }));
    let id = app.call_part().active.unwrap().id;
    let _ = app.update_call(CallMsg::Action(CallAction::LocalDescription {
        call_id: id.clone(),
        sdp: "v=0".into(),
    }));
    let _ = app.update_call(CallMsg::Http(
        app.generation,
        id.clone(),
        Op::Join,
        Ok(json!({"links": {"leave": "https://conv.example/l",
            "addParticipantAndModality": "https://conv.example/addm"}})),
    ));
    let pushed = |name: &str, body: serde_json::Value| teams_realtime::PushedRequest {
        method: "POST".into(),
        url: String::new(),
        path: format!("/callAgent/sess/{id}/{name}"),
        headers: Default::default(),
        body: body.to_string(),
    };
    let _ = app.call_pushed(&pushed(
        "conversation/rosterUpdate/",
        json!({"participants": {
            "8:orgid:me": {"displayName": "Moi", "role": "Presenter"},
            "8:orgid:o": {"displayName": "Olga", "role": "Organizer"}}}),
    ));
    let _ = app.call_pushed(&pushed("call/acceptance/", json!({})));
    let call = app.call_part().active.unwrap();
    assert!(call.can_record);
    assert_eq!(call.recording, RecordingView::None);

    let _ = app.update_call(CallMsg::Action(CallAction::StartRecording));
    assert_eq!(app.call_part().active.unwrap().recording, RecordingView::Starting);
    let _ = app.update_call(CallMsg::RecorderToken(app.generation, id.clone(), Ok("aad".into())));
    assert_eq!(
        app.call_part().active.unwrap().recording,
        RecordingView::Starting,
        "the bot is being invited"
    );

    let _ = app.call_pushed(&pushed(
        "conversation/rosterUpdate/",
        json!({"participants": {payload::recording::BOT_MRIS[0]: {
            "displayName": "Recorder",
            "endpoints": {"bot": {"call": {}, "endpointMetadata": {
                "commandUrl": "https://bot.example/command",
                "processingModes": {"recording": {"state": "Active", "userId": "8:orgid:o"}}}}}}}}),
    ));
    let call = app.call_part().active.unwrap();
    assert_eq!(call.recording, RecordingView::Active { by: Some("Olga".into()) });
    assert_eq!(call.participants.len(), 1, "the recorder bot is not a participant");

    let _ = app.update_call(CallMsg::Action(CallAction::StopRecording));
    assert_eq!(app.call_part().active.unwrap().recording, RecordingView::Stopping);
}

#[test]
fn recording_needs_a_connected_call() {
    let mut app = ready();
    let _ = app.update_call(CallMsg::Action(CallAction::Start {
        mri: "8:orgid:bob".into(),
        video: false,
    }));
    let id = app.call_part().active.unwrap().id;
    // No session yet: nothing to record.
    let _ = app.update_call(CallMsg::Action(CallAction::StartRecording));
    assert_eq!(app.call_part().active.unwrap().recording, RecordingView::None);
    assert!(!app.call_part().active.unwrap().can_record);
    let _ = app.update_call(CallMsg::RecorderToken(app.generation, id, Err("x".into())));
    assert_eq!(app.call_part().active.unwrap().recording, RecordingView::None, "nothing shown before the call connects");
}

use super::*;
/// A banner opening its chat from a channel: going back to chats selects the last chat, which must not land after the banner's own.
#[test]
fn follow_ups_apply_depth_first_like_nested_calls() {
    let mut app = App::empty();
    *app.chats = ["a", "b"]
        .map(|id| serde_json::from_value(serde_json::json!({ "id": id })).unwrap())
        .to_vec();
    *app.selected = Some(Selection::Channel {
        team: "team".into(),
        channel: "channel".into(),
    });
    app.last_chat = Some("b".into());
    let _ = app.update(Msg::Notifications(NotificationsMsg::Action(
        notifications::NotificationsAction::Open {
            conversation_id: "a".into(),
            reply_chain_id: None,
        },
    )));
    assert_eq!(app.selected, Some(Selection::Chat("a".into())));
    assert_eq!(*app.nav, Nav::Chat);
}
fn feed_page(ids: std::ops::Range<u32>, cursor: &str) -> MessagesPage {
    MessagesPage {
        messages: ids
            .map(|i| {
                serde_json::from_value(serde_json::json!({
                    "id": format!("m{i:04}"), "from": "8:orgid:peer", "content": "<p>x</p>",
                    "originalarrivaltime": format!("2026-10-15T10:00:00.{i:04}Z"),
                    "composetime": format!("2026-10-15T10:00:00.{i:04}Z"),
                }))
                .unwrap()
            })
            .collect(),
        metadata: teams_chat::PageMetadata {
            sync_state: Some("https://x/messages?syncState=delta".into()),
            backward_link: (!cursor.is_empty())
                .then(|| format!("https://x/messages?startTime=1&syncState={cursor}&pageSize=50")),
            ..Default::default()
        },
        ..Default::default()
    }
}
fn feed_loaded(app: &mut App, append: bool, page: MessagesPage) {
    let _ = app.update(ConversationMsg::FeedLoaded {
        generation: app.generation,
        selection: Selection::Chat("a".into()),
        append,
        result: Ok(page),
    });
}
/// At the bottom a long session keeps only the latest messages and "load more" restarts from the latest page; scrolled up, nothing is dropped.
#[test]
fn the_open_feed_keeps_its_latest_messages_until_the_reader_goes_up() {
    let mut app = App::empty();
    *app.chats = vec![serde_json::from_value(serde_json::json!({"id": "a"})).unwrap()];
    let _ = app.update(Msg::Select(Selection::Chat("a".into())));
    feed_loaded(&mut app, false, feed_page(0..50, "older-than-m0000"));
    let _ = app.conv.rendered().get(&app.conv.messages()[0]);
    for start in (50..400).step_by(50) {
        feed_loaded(
            &mut app,
            false,
            feed_page(start..start + 50, "older-than-latest"),
        );
    }
    let messages = app.conv.messages();
    assert_eq!(messages.len(), conversation::MAX_MESSAGES);
    assert_eq!(messages[0].id, "m0100");
    assert_eq!(messages.last().unwrap().id, "m0399");
    assert!(
        app.conv.has_more(),
        "the dropped messages can be loaded again"
    );
    feed_loaded(&mut app, true, feed_page(50..100, "older-than-m0050"));
    feed_loaded(&mut app, true, feed_page(0..50, "older-than-m0000"));
    feed_loaded(&mut app, false, feed_page(400..450, "older-than-latest"));
    assert_eq!(
        app.conv.messages().len(),
        450,
        "the reader is up in the history"
    );
    let _ = app.update(Msg::Select(Selection::Chat("a".into())));
    feed_loaded(&mut app, false, feed_page(0..450, "older-than-latest"));
    assert_eq!(
        app.conv.messages().len(),
        conversation::MAX_MESSAGES,
        "reopened"
    );
}
/// The delta link in `syncState` does not page back: without a `backwardLink` there is nothing older to load.
#[test]
fn older_messages_page_from_the_backward_link_only() {
    for (cursor, more) in [("", false), ("older", true)] {
        let mut app = App::empty();
        *app.chats = vec![serde_json::from_value(serde_json::json!({"id": "a"})).unwrap()];
        let _ = app.update(Msg::Select(Selection::Chat("a".into())));
        feed_loaded(&mut app, false, feed_page(0..50, cursor));
        assert_eq!(app.conv.has_more(), more, "backward cursor {cursor:?}");
    }
}
#[test]
fn the_activity_feed_keeps_its_newest_items_until_the_reader_asks_for_more() {
    let page = |ids: std::ops::Range<u32>| {
        let mut page = feed_page(ids, "");
        page.metadata.backward_link = Some("https://x/messages?syncState=older".into());
        page
    };
    let mut app = App::empty();
    let loaded = |app: &mut App, append, page| {
        let _ = app.update(Msg::Activity(ActivityMsg::Loaded(
            app.generation,
            append,
            Ok(page),
        )));
    };
    loaded(&mut app, false, page(0..50));
    for start in (50..300).step_by(50) {
        loaded(&mut app, false, page(start..start + 50));
    }
    assert_eq!(app.activity.messages.len(), 200);
    assert_eq!(app.activity.messages[0].id, "m0100");
    assert_eq!(app.activity.cursor.as_deref(), Some("older"));
    loaded(&mut app, true, page(50..100));
    loaded(&mut app, false, page(300..350));
    assert_eq!(app.activity.messages.len(), 300);
}
#[test]
fn an_opened_channel_activity_stays_on_the_activity_screen() {
    let mut app = App::empty();
    *app.teams = vec![
        serde_json::from_value(serde_json::json!({
            "id": "team", "channels": [{"id": "19:c@thread.tacv2"}]
        }))
        .unwrap(),
    ];
    *app.nav = Nav::Activity;
    app.activity.messages = vec![
        serde_json::from_value(serde_json::json!({
            "id": "n1", "properties": {"activity": {"sourceThreadId": "19:c@thread.tacv2"}}
        }))
        .unwrap(),
    ];
    let _ = app.update(Msg::Activity(ActivityMsg::Open("n1".into())));
    assert_eq!(
        app.selected,
        Some(Selection::Channel {
            team: "team".into(),
            channel: "19:c@thread.tacv2".into(),
        })
    );
    assert_eq!(*app.nav, Nav::Activity);
}
#[tokio::test]
async fn recovery_starts_a_new_generation_and_keeps_the_shared_caches() {
    let (mut app, _) = App::boot(true);
    let avatars = app.avatars.clone();
    let handoff = app.handoff.clone();
    let generation = app.generation;
    *app.search = "lost".into();
    let _ = app.recover(true);
    assert!(Arc::ptr_eq(&avatars, &app.avatars));
    assert!(
        Arc::ptr_eq(&handoff, &app.handoff),
        "Swift takes images from it"
    );
    assert_eq!(app.generation, generation + 1);
    assert!(app.search.is_empty());
    assert!(app.error.is_some());
    assert!(app.signed_in());
}
#[test]
fn hidden_regular_chats_reappear_after_messages_and_favorites_are_kept() {
    use serde_json::json;
    let startup: Startup = serde_json::from_value(json!({
        "chats": [
            {"id":"dm", "hidden":true, "isOneOnOne":true, "chatType":"chat",
                "lastMessage":{"id":"new", "originalArrivalTime":"2026-10-15T09:10:17.3150000Z"}},
            {"id":"group", "hidden":true, "isOneOnOne":false, "chatType":"chat",
                "hiddenTime":"2026-10-01T00:00:00Z",
                "lastMessage":{"id":"new", "composeTime":"2026-10-15T08:00:00Z"}},
            {"id":"still-hidden", "hidden":true, "isOneOnOne":true,
                "unpinnedTime":"2026-10-15T10:00:00Z",
                "lastMessage":{"originalArrivalTime":"2026-10-15T08:00:00Z"}},
            {"id":"favorite", "hidden":true},
            {"id":"deleted", "isConversationDeleted":true},
            {"id":"hidden-meeting", "hidden":true, "isOneOnOne":false,"chatSubType":"Meeting",
                "lastMessage":{"originalArrivalTime":"2026-10-15T08:00:00Z"}},
            {"id":"channel@thread.tacv2"},
            {"id":"known-channel"}
        ],
        "teams":[{"id":"team", "channels":[{"id":"known-channel"}]}],
        "conversationFolders":{"conversationFolders":[{"folderType":"Favorites",
            "conversationFolderItems":[{"conversationId":"favorite"},{"conversationId":"deleted"}]}]}
    })).unwrap();
    let mut app = App::empty();
    let _ = app.update(Msg::WorkspaceLoaded(0, Ok(workspace(startup))));
    let ids: HashSet<_> = app.chats.iter().map(|c| c.id.as_str()).collect();
    assert_eq!(ids, HashSet::from(["dm", "group", "favorite"]));
    let page: ChatsPage = serde_json::from_value(json!({"chats":[
        {"id":"paged-dm", "hidden":true, "isOneOnOne":true,
            "lastMessage":{"originalArrivalTime":"2026-10-15T09:00:00Z"}},
        {"id":"paged-channel@thread.tacv2"}, {"id":"known-channel"}
    ]}))
    .unwrap();
    let _ = app.update(Msg::MoreChatsLoaded(0, Ok(page)));
    assert_eq!(app.chats.len(), 4);
    assert!(app.chats.iter().any(|c| c.id == "paged-dm"));
}
#[test]
fn direct_chat_title_shows_only_the_peer_with_either_member_order() {
    use serde_json::json;
    let me = "8:orgid:me";
    for members in [
        json!([{"id":me,"friendlyName":"Moi"},{"id":"8:orgid:peer","friendlyName":"Alice"}]),
        json!([{"id":"8:orgid:peer","friendlyName":"Alice"},{"id":me,"friendlyName":"Moi"}]),
    ] {
        let chat: Conversation = serde_json::from_value(json!({
            "id":"private", "isOneOnOne":true, "members":members
        }))
        .unwrap();
        assert_eq!(chat_title(&chat, me), "Alice");
        assert!(!chat.is_group());
        assert_eq!(
            existing_chat(
                &[chat],
                &HashMap::new(),
                me,
                &[Person {
                    mri: "8:orgid:peer".into(),
                    ..Default::default()
                }]
            ),
            Some("private".into())
        );
    }
}
/// A roster lookup reads the directory in place, keeping only chats' members rather than copies of the directory or chats.
#[tokio::test]
async fn roster_lookups_share_the_directory_and_keep_only_members() {
    let (mut app, _) = App::boot(true);
    Arc::make_mut(&mut app.profiles).insert(
        "8:orgid:peer".into(),
        Person {
            mri: "8:orgid:peer".into(),
            ..Default::default()
        },
    );
    *app.chats =
        vec![serde_json::from_value(serde_json::json!({"id": "g", "chatType": "chat"})).unwrap()];
    let task = app.resolve_chats();
    assert_eq!(task.0.len(), 1);
    assert_eq!(
        Arc::strong_count(&app.profiles),
        2,
        "shared with the lookup"
    );
    drop(task);
    let found: Conversation = serde_json::from_value(serde_json::json!({
        "id": "g", "members": [{"id": "8:orgid:peer"}], "rosterVersion": 3,
        "lastMessage": {"id": "m", "content": "<p>Un long message</p>"},
        "threadProperties": {"topic": "Projet"}
    }))
    .unwrap();
    let _ = app.update(Msg::ChatsResolved(
        app.generation,
        conversations::Resolution {
            chats: vec![found],
            attempted: vec!["g".into()],
            ..Default::default()
        },
    ));
    let details = &app.details["g"];
    assert_eq!(details.members.len(), 1);
    assert_eq!(details.roster_version, Some(serde_json::json!(3)));
    assert_eq!(app.chats[0].title(), Some("Projet"));
}
#[test]
fn resolved_external_profile_updates_sidebar_without_waiting_for_rosters() {
    let mut app = App::empty();
    *app.me = "8:orgid:me".into();
    *app.chats = vec![
        serde_json::from_value(serde_json::json!({
            "id":"external", "isOneOnOne":true,
            "members":[{"id":"8:orgid:me"},{"id":"8:orgid:external"}]
        }))
        .unwrap(),
    ];
    assert_eq!(chat_title(&app.chats[0], &app.me), "Discussion");
    let _ = app.update(Msg::ProfilesLoaded(
        0,
        Ok(vec![Person {
            mri: "8:orgid:external".into(),
            display_name: "Bruno Externe".into(),
            ..Default::default()
        }]),
    ));
    assert_eq!(chat_title(&app.chats[0], &app.me), "Bruno Externe");
    assert!(!app.chats[0].is_group());
}
#[test]
fn account_profile_updates_visible_fields_and_ignores_late_results_after_logout() {
    let mut app = App::empty();
    *app.me = "8:orgid:me".into();
    let profile = Person {
        mri: "8:orgid:me".into(),
        display_name: "John Doe".into(),
        email: "john.doe@example.com".into(),
        ..Default::default()
    };
    let _ = app.update(Msg::MyProfileLoaded(0, Ok(profile.clone())));
    assert_eq!(app.display_name, "John Doe");
    assert_eq!(app.account_email, "john.doe@example.com");
    assert_eq!(app.me, "8:orgid:me");
    let _ = app.update(Msg::MyProfileLoaded(0, Err("offline".into())));
    assert_eq!(app.display_name, "John Doe");
    let _ = app.update(Msg::SignOut);
    let _ = app.update(Msg::MyProfileLoaded(0, Ok(profile)));
    assert!(app.display_name.is_empty());
    assert!(app.account_email.is_empty());
}
fn selection(id: &str) -> Selection {
    Selection::Chat(id.into())
}
/// Sign-in builds its URL at once when the application ID is already known.
fn known_client_id() {
    assert!(teams_auth::client_id::remember(
        "0c6e1f4a-3b1d-4c8e-9f2a-7d5b6a4c3e21"
    ));
}
#[test]
fn native_login_asks_the_shell_for_a_window_and_reports_cancellation() {
    known_client_id();
    let mut app = App::empty();
    let task = app.update(Msg::SignIn);
    assert!(task.effects().any(|e| matches!(
        e,
        Effect::OpenLogin { url } if url.starts_with("https://login.microsoftonline.com/")
    )));
    assert!(*app.signing_in);
    assert!(app.update(Msg::SignIn).effects().next().is_none());
    let _ = app.update(Msg::LoginCancelled);
    assert!(!*app.signing_in);
    assert!(app.error.is_some());
    let _ = app.update(Msg::LoginReturned(
        "https://teams.microsoft.com/v2#code=x".into(),
    ));
    assert!(
        !*app.signing_in,
        "a late return without an attempt is ignored"
    );
}
#[test]
fn an_expired_saved_session_first_tries_a_silent_sign_in() {
    known_client_id();
    let mut app = App::empty();
    let task = app.update(Msg::SessionRestored(
        0,
        Err(CoreError::SessionExpired("expirée".into())),
    ));
    assert!(task.effects().any(|e| matches!(
        e,
        Effect::SilentLogin { url } if url.contains("prompt=none")
    )));
    assert!(*app.signing_in);
    assert!(app.error.is_none());
    let _ = app.update(Msg::SilentLoginFailed);
    assert!(!*app.signing_in);
    assert!(app.error.is_some());
    // A late answer of the invisible view no longer has an attempt to complete.
    assert!(
        app.update(Msg::LoginReturned(
            "https://teams.microsoft.com/v2#code=x".into()
        ))
        .effects()
        .next()
        .is_none()
    );
}
#[test]
fn a_silent_sign_in_failing_at_launch_falls_back_to_the_sign_in_page() {
    known_client_id();
    let mut app = App::empty();
    let _ = app.update(Msg::SessionRestored(
        0,
        Err(CoreError::SessionExpired("expirée".into())),
    ));
    let _ = app.update(Msg::SilentRenewed(0, Err("login_required".into())));
    assert!(!*app.signing_in);
    assert!(app.error.is_some());
    // The interactive window still opens afterwards.
    assert!(app.update(Msg::SignIn).effects().any(|e| matches!(
        e,
        Effect::OpenLogin { url } if url.contains("prompt=select_account")
    )));
}
#[tokio::test]
async fn a_silent_renewal_clears_the_expired_session_banner_and_reloads() {
    let (mut app, _) = App::boot(true);
    *app.error = Some("Session Microsoft expirée. Reconnectez-vous.".into());
    app.resolved_ids.insert("g".into());
    let task = app.update(Msg::SilentRenewed(app.generation, Ok(None)));
    assert!(app.error.is_none());
    assert!(app.resolved_ids.is_empty(), "chats are resolved again");
    drop(task);
    assert!(*app.loading_workspace, "the workspace reloads");
}
#[test]
fn signing_out_deletes_the_microsoft_cookies() {
    let mut app = App::empty();
    assert!(
        app.update(Msg::SignOut)
            .effects()
            .any(|e| matches!(e, Effect::ForgetLogin))
    );
}
#[test]
fn only_core_side_draft_changes_reset_the_editor_and_unchanged_parts_are_not_resent() {
    let mut app = App::empty();
    let mut snapshot = view::Snapshot::default();
    assert!(snapshot.diff(&app, view::Parts::ALL).shell.is_some());
    assert!(snapshot.diff(&app, view::Parts::ALL).is_empty());
    let _ = app.update(Msg::Select(selection("alice")));
    let rev = app.conv.draft_rev();
    let _ = app.update(Msg::DraftChanged("typed".into()));
    assert_eq!(app.conv.draft_rev(), rev);
    let update = snapshot.diff(&app, view::Parts::ALL);
    assert_eq!(update.composer.unwrap().draft, "typed");
    assert!(update.sidebar.is_none());
    let _ = app.update(ConversationMsg::InsertEmoji("🎉".into()));
    assert_eq!(app.conv.draft(), "typed 🎉");
    assert!(app.conv.draft_rev() > rev);
    let _ = app.update(Msg::Select(selection("bob")));
    assert!(app.conv.draft().is_empty());
    let _ = app.update(Msg::Select(selection("alice")));
    assert_eq!(app.conv.draft(), "typed 🎉");
}
#[test]
fn an_unread_chat_marks_its_first_new_message_from_others() {
    let message = |id: &str, from: &str, at: &str| -> Message {
        serde_json::from_value(serde_json::json!({
            "id": id, "from": from, "originalarrivaltime": at, "content": id,
        }))
        .unwrap()
    };
    let mut app = App::empty();
    *app.me = "8:orgid:me".into();
    *app.chats = vec![
        serde_json::from_value(serde_json::json!({
            "id": "alice", "isRead": false,
            "consumptionHorizon": {"originalArrivalTime": 1_700_000_001_000_i64},
        }))
        .unwrap(),
    ];
    let _ = app.update(Msg::Select(selection("alice")));
    *app.conv.test_messages() = vec![
        message("1", "8:orgid:alice", "2023-11-14T22:13:20.000Z"),
        message("2", "8:orgid:me", "2023-11-14T22:13:22.000Z"),
        message("3", "8:orgid:alice", "2023-11-14T22:13:23.000Z"),
    ];
    let first = |app: &App| app.conversation_part().view.unwrap().first_unread;
    assert_eq!(first(&app).as_deref(), Some("3"));
    // Without a horizon only the last message counts as new.
    let horizon = view::read_horizon(
        &serde_json::from_value(serde_json::json!({
            "id": "bob", "isRead": false,
            "lastMessage": {"originalarrivaltime": "2023-11-14T22:13:23.000Z"},
        }))
        .unwrap(),
    );
    assert_eq!(horizon, Some(1_700_000_002_999));
    let _ = app.update(Msg::Select(selection("bob")));
    assert_eq!(first(&app), None);
}
#[test]
fn notices_close_one_by_one_and_apart_from_the_error() {
    let mut app = App::empty();
    *app.error = Some("erreur".into());
    *app.notice = Some("avis".into());
    *app.participants_notice = Some("participants".into());
    let _ = app.update(Msg::DismissNotice("avis".into()));
    assert_eq!(app.notice, None);
    assert!(app.error.is_some() && app.participants_notice.is_some());
    let _ = app.update(Msg::DismissError);
    assert_eq!(app.error, None);
    assert!(app.participants_notice.is_some());
}
#[test]
fn a_new_chat_with_the_same_people_reopens_the_existing_one() {
    let chat = |id: &str, one: bool, members: &[&str]| -> Conversation {
        serde_json::from_value(serde_json::json!({
            "id": id, "isOneOnOne": one,
            "members": members.iter().map(|m| serde_json::json!({"id": m})).collect::<Vec<_>>(),
        }))
        .unwrap()
    };
    let person = |mri: &str| Person {
        mri: mri.into(),
        ..Default::default()
    };
    let mut app = App::empty();
    *app.me = "8:orgid:me".into();
    *app.client = Some(ChatClient::new(Arc::new(AppAuthenticator::Demo(
        StaticAuthenticator::new(),
    ))));
    let group = chat(
        "group",
        false,
        &["8:orgid:ME", "8:orgid:alice", "8:orgid:bob"],
    );
    let partial = chat(
        "partial",
        false,
        &["8:orgid:me", "8:orgid:alice", "8:orgid:carol"],
    );
    app.details.insert(
        "group".into(),
        conversations::ChatDetails::of(group.clone()),
    );
    *app.chats = vec![
        chat("dm", true, &["8:orgid:me", "8:orgid:Alice"]),
        group,
        partial,
    ];
    let open = |app: &mut App, people: &[&str], title: &str| {
        let _ = app.update(Msg::NewChat);
        for p in people {
            let _ = app.update(Msg::ChoosePerson(person(p)));
        }
        let _ = app.update(Msg::NewTitle(title.into()));
        let _ = app.update(Msg::CreateChat);
    };
    open(&mut app, &["8:orgid:alice"], "");
    assert_eq!(app.selected, Some(Selection::Chat("dm".into())));
    assert!(app.new_chat.is_none());
    open(&mut app, &["8:orgid:bob", "8:orgid:alice"], "");
    assert_eq!(app.selected, Some(Selection::Chat("group".into())));
    // The roster of "partial" was never fetched: it may hide other members.
    open(&mut app, &["8:orgid:alice", "8:orgid:carol"], "");
    assert!(app.new_chat.as_ref().is_some_and(|d| d.creating));
    // A named group is a new chat, even with the same people.
    open(&mut app, &["8:orgid:alice", "8:orgid:bob"], "Projet");
    assert!(app.new_chat.as_ref().is_some_and(|d| d.creating));
}
#[test]
fn a_new_chat_with_only_yourself_opens_your_notes() {
    let mut app = App::empty();
    *app.me = "8:orgid:ABC".into();
    *app.client = Some(ChatClient::new(Arc::new(AppAuthenticator::Demo(
        StaticAuthenticator::new(),
    ))));
    let _ = app.update(Msg::NewChat);
    let _ = app.update(Msg::ChoosePerson(Person {
        mri: "8:orgid:abc".into(),
        display_name: "Moi".into(),
        ..Default::default()
    }));
    let task = app.update(Msg::CreateChat);
    assert!(app.new_chat.is_none());
    assert_eq!(app.selected, Some(Selection::Chat(NOTES.into())));
    assert_eq!(app.title(), "Vous (notes personnelles)");
    assert!(task.effects().next().is_none());
}
#[test]
fn bots_and_connectors_are_named_like_teams_names_them() {
    use serde_json::json;
    let mut app = App::empty();
    let mut channel = Channel::default();
    channel.extra.insert(
        "connectorProfiles".into(),
        json!([{"id": "abc-123", "displayName": "Outil ABS", "connectorType": "IncomingWebhook"}]),
    );
    *app.teams = vec![Team {
        id: "team".into(),
        channels: vec![channel],
        ..Default::default()
    }];
    let from = |mri: &str| format!("https://x/v1/users/ME/contacts/{mri}");
    let webhook = Message {
        from: from("28:integration:abc-123"),
        ..Default::default()
    };
    assert_eq!(app.message_view(&webhook).sender, "Outil ABS");
    let mut bot = Message {
        from: from("28:bot-id"),
        imdisplayname: Some("Bot".into()),
        ..Default::default()
    };
    bot.properties.insert(
        "fromAppMetadata".into(),
        json!(r#"{"id":"app-id","name":"Planner"}"#),
    );
    assert_eq!(app.message_view(&bot).sender, "Planner");
    let unknown = Message {
        from: from("28:other"),
        ..Default::default()
    };
    assert_eq!(app.message_view(&unknown).sender, "Application");
}
#[test]
fn unnamed_authors_never_show_their_id_and_take_the_directory_name() {
    let mut app = App::empty();
    let id = "8:orgid:00000000-0000-0000-0000-0000000000a1";
    let message = Message {
        id: "1".into(),
        from: format!("https://x/v1/users/ME/contacts/{id}"),
        imdisplayname: Some(" ".into()),
        ..Default::default()
    };
    assert_eq!(app.message_view(&message).sender, "Expéditeur inconnu");
    let _ = app.update(Msg::ProfilesLoaded(
        0,
        Ok(vec![Person {
            mri: id.into(),
            display_name: "Dnoob".into(),
            ..Default::default()
        }]),
    ));
    assert_eq!(app.message_view(&message).sender, "Dnoob");
    let bot = Message {
        from: "https://x/v1/users/ME/contacts/28:bot".into(),
        ..Default::default()
    };
    assert_eq!(app.message_view(&bot).sender, "Application");
}
#[test]
fn previews_on_screen_load_past_the_budget_in_place_of_those_furthest_away() {
    let mut app = App::empty();
    *app.client = Some(ChatClient::new(Arc::new(AppAuthenticator::Demo(
        StaticAuthenticator::new(),
    ))));
    let src = |i: usize| format!("https://example.com/{i}.png");
    let count = media::MAX_IMAGES + 12;
    let messages = (0..count)
        .map(|i| {
            serde_json::from_value(serde_json::json!({
                "id": format!("m{i}"), "originalarrivaltime": format!("2026-10-15T10:{:02}:00Z", i),
                "content": format!("<p><img src=\"{}\"></p>", src(i)),
            }))
            .unwrap()
        })
        .collect();
    let _ = app.update(Msg::Select(selection("a")));
    let _ = app.update(Msg::from(ConversationMsg::FeedLoaded {
        generation: 0,
        selection: selection("a"),
        append: false,
        result: Ok(MessagesPage {
            messages,
            ..Default::default()
        }),
    }));
    let finish = |app: &mut App| loop {
        let loading: Vec<String> = app
            .conv
            .images()
            .iter()
            .filter(|(_, s)| matches!(s, media::ImageState::Loading))
            .map(|(src, _)| src.clone())
            .collect();
        if loading.is_empty() {
            break;
        }
        for src in loading {
            let _ = app.update(Msg::from(ConversationMsg::ImageLoaded {
                generation: 0,
                selection: selection("a"),
                src,
                result: Ok(media::LoadedImage {
                    width: 1,
                    height: 1,
                    data: Arc::new(vec![1]),
                    bytes: 1,
                }),
            }));
        }
    };
    finish(&mut app);
    // The newest fill the budget; the oldest wait, shown as loading rather than refused.
    assert_eq!(app.conv.images().len(), media::MAX_IMAGES);
    assert!(app.conv.images().contains_key(&src(count - 1)));
    assert!(!app.conv.images().contains_key(&src(0)));
    let html = crate::html::InlineImage {
        src: src(1),
        alt: String::new(),
        width: None,
        height: None,
    };
    assert!(matches!(
        app.image_view(&html).status,
        crate::ImageStatus::Loading
    ));

    // Scrolled to the top: its previews take the places of those furthest from it.
    let _ = app.update(Msg::from(ConversationMsg::Shown(vec![
        "m0".into(),
        "m1".into(),
    ])));
    finish(&mut app);
    let images = app.conv.images();
    assert_eq!(images.len(), media::MAX_IMAGES);
    assert!(matches!(
        images.get(&src(0)),
        Some(media::ImageState::Ready(_))
    ));
    assert!(matches!(
        images.get(&src(1)),
        Some(media::ImageState::Ready(_))
    ));
    let oldest_newest = count - media::MAX_IMAGES;
    assert!(!images.contains_key(&src(oldest_newest)));
    assert!(!images.contains_key(&src(oldest_newest + 1)));
    assert!(images.contains_key(&src(count - 1)));
}
#[test]
fn previews_survive_switching_conversations_until_the_budget_needs_them() {
    let mut app = App::empty();
    *app.client = Some(ChatClient::new(Arc::new(AppAuthenticator::Demo(
        StaticAuthenticator::new(),
    ))));
    let src = "https://example.com/a.png";
    let feed = |app: &mut App, chat: &str| {
        let message: Message = serde_json::from_value(serde_json::json!({
            "id": format!("{chat}-1"), "originalarrivaltime": "2026-10-15T10:00:00Z",
            "content": format!("<p><img src=\"{src}\"></p>"),
        }))
        .unwrap();
        let page = MessagesPage {
            messages: vec![message],
            ..Default::default()
        };
        let _ = app.update(Msg::from(ConversationMsg::FeedLoaded {
            generation: 0,
            selection: selection(chat),
            append: false,
            result: Ok(page),
        }));
    };
    let loaded = |bytes| {
        Ok(media::LoadedImage {
            width: 1,
            height: 1,
            data: Arc::new(vec![1; bytes]),
            bytes,
        })
    };
    let _ = app.update(Msg::Select(selection("a")));
    feed(&mut app, "a");
    assert!(matches!(
        app.conv.images().get(src),
        Some(media::ImageState::Loading)
    ));
    let _ = app.update(Msg::from(ConversationMsg::ImageLoaded {
        generation: 0,
        selection: selection("a"),
        src: src.into(),
        result: loaded(16),
    }));
    assert!(app.take_image(src).is_some());

    let _ = app.update(Msg::Select(selection("b")));
    assert!(app.conv.images().is_empty());
    assert_eq!(app.kept_image_srcs(), [src]);

    // Back to the first chat: the preview is ready at once, with no new download.
    let _ = app.update(Msg::Select(selection("a")));
    feed(&mut app, "a");
    assert!(matches!(
        app.conv.images().get(src),
        Some(media::ImageState::Ready(_))
    ));
    assert_eq!(app.take_image(src), None, "Swift already holds the picture");

    // A new preview elsewhere that needs the whole budget evicts the retained one.
    let _ = app.update(Msg::Select(selection("b")));
    let other = "https://example.com/b.png";
    app.conv
        .test_images()
        .insert(other.into(), media::ImageState::Loading);
    let _ = app.update(Msg::from(ConversationMsg::ImageLoaded {
        generation: 0,
        selection: selection("b"),
        src: other.into(),
        result: loaded(media::MAX_IMAGE_BYTES),
    }));
    assert!(matches!(
        app.conv.images().get(other),
        Some(media::ImageState::Ready(_))
    ));
    assert_eq!(app.kept_image_srcs(), [other]);
}
#[test]
fn preview_files_are_handed_over_once_and_stay_in_the_budget() {
    let mut app = App::empty();
    app.image_ready(
        "a".into(),
        media::LoadedImage {
            width: 2,
            height: 1,
            data: Arc::new(vec![7; 8]),
            bytes: 8,
        },
    );
    assert_eq!(app.take_image("a"), Some((2, 1, vec![7; 8])));
    assert_eq!(app.take_image("a"), None);
    assert!(matches!(
        app.conv.images().get("a"),
        Some(media::ImageState::Ready(i)) if i.bytes == 8 && i.width == 2
    ));
    assert_eq!(app.take_image("missing"), None);
}
#[test]
fn an_unchanged_background_refresh_sends_nothing_to_the_shell() {
    let mut app = App::empty();
    *app.chats = vec![Conversation {
        id: "alice".into(),
        ..Default::default()
    }];
    *app.teams = vec![Team {
        id: "team".into(),
        ..Default::default()
    }];
    let _ = app.update(Msg::Select(selection("alice")));
    let page = || {
        Ok(MessagesPage {
            messages: vec![Message {
                id: "1".into(),
                content: "<p>Bonjour</p>".into(),
                ..Default::default()
            }],
            ..Default::default()
        })
    };
    let _ = app.update(Msg::from(ConversationMsg::FeedLoaded {
        generation: 0,
        selection: selection("alice"),
        append: false,
        result: page(),
    }));
    let mut snapshot = view::Snapshot::default();
    let _ = snapshot.diff(&app, view::Parts::ALL);
    // What a tick does: feed and workspace requests start, then the same data returns.
    *app.conv.test_loading() = true;
    *app.loading_workspace = true;
    assert!(snapshot.diff(&app, view::Parts::ALL).is_empty());
    let _ = app.update(Msg::from(ConversationMsg::FeedLoaded {
        generation: 0,
        selection: selection("alice"),
        append: false,
        result: page(),
    }));
    *app.loading_workspace = false;
    assert!(snapshot.diff(&app, view::Parts::ALL).is_empty());
}
#[test]
fn drafts_are_isolated_between_chats_channels_and_reply_threads() {
    let mut app = App::empty();
    let _ = app.update(Msg::Select(selection("alice")));
    app.conv.draft_typed("Private draft".into());
    let channel = Selection::Channel {
        team: "team".into(),
        channel: "general".into(),
    };
    let _ = app.update(Msg::Select(channel.clone()));
    app.conv.draft_typed("Channel post".into());
    let _ = app.update(ConversationMsg::OpenReply("root".into()));
    app.conv.draft_typed("Thread reply".into());
    let _ = app.update(ConversationMsg::CloseReply);
    assert_eq!(app.conv.draft(), "Channel post");
    let _ = app.update(ConversationMsg::OpenReply("root".into()));
    assert_eq!(app.conv.draft(), "Thread reply");
    let _ = app.update(Msg::Select(selection("alice")));
    assert_eq!(app.conv.draft(), "Private draft");
}
#[test]
fn logout_ignores_late_results_and_removes_private_drafts() {
    let mut app = App::empty();
    *app.selected = Some(selection("alice"));
    app.conv.draft_typed("secret".into());
    *app.me = "alice".into();
    app.conv.save_draft(app.selected.as_ref());
    let old = app.generation;
    let _ = app.update(Msg::SignOut);
    let _ = app.update(Msg::from(ConversationMsg::FeedLoaded {
        generation: old,
        selection: selection("alice"),
        append: false,
        result: Ok(MessagesPage {
            messages: vec![Message {
                content: "private".into(),
                ..Default::default()
            }],
            ..Default::default()
        }),
    }));
    assert!(app.me.is_empty());
    assert!(app.conv.messages().is_empty());
    assert!(app.conv.test_drafts().is_empty());
    assert!(app.conv.draft().is_empty());
}
#[test]
fn send_failure_keeps_the_draft_and_history_refresh_does_not_discard_older_messages() {
    let mut app = App::empty();
    *app.selected = Some(selection("alice"));
    app.conv.draft_typed("unsent".into());
    let _ = app.update(Msg::Sent {
        generation: 0,
        selection: selection("alice"),
        reply: None,
        content: "unsent".into(),
        ids: vec![],
        result: Err("offline".into()),
    });
    assert_eq!(app.conv.draft(), "unsent");
    assert!(app.error.is_some());
    *app.conv.test_messages() = vec![Message {
        id: "old".into(),
        content: "older page".into(),
        ..Default::default()
    }];
    let _ = app.update(Msg::from(ConversationMsg::FeedLoaded {
        generation: 0,
        selection: selection("alice"),
        append: false,
        result: Ok(MessagesPage {
            messages: vec![Message {
                id: "new".into(),
                content: "new page".into(),
                ..Default::default()
            }],
            ..Default::default()
        }),
    }));
    assert_eq!(app.conv.messages().len(), 2);
}
#[test]
fn startup_merges_top_level_channels_and_discards_deleted_items() {
    let data:Startup=serde_json::from_value(serde_json::json!({"teams":[{"id":"team","displayName":"Team","channels":[]}],"channels":[{"id":"general","parentTeamId":"team","displayName":"Général","isGeneral":true},{"id":"deleted","parentTeamId":"team","isDeleted":true}],"chats":[{"id":"hidden","hidden":true},{"id":"private","title":"Private"}]})).unwrap();
    let workspace = workspace(data);
    assert_eq!(workspace.chats.len(), 1);
    assert_eq!(workspace.teams[0].channels.len(), 1);
    assert_eq!(workspace.teams[0].channels[0].id, "general");
}
#[tokio::test]
async fn demo_supports_private_chat_posts_replies_edit_delete_and_people_search() {
    let addr = mock::spawn();
    let base = Url::parse(&format!("http://{addr}/")).unwrap();
    let auth = StaticAuthenticator::new()
        .with_service(Service::ChatService, base.clone())
        .with_service(Service::Csa, base.clone())
        .with_service(Service::MiddleTier, base);
    let client = ChatClient::new(Arc::new(auth));
    let startup = client.startup().await.unwrap();
    assert_eq!(startup.teams.len(), 2);
    assert!(!startup.chats.is_empty());
    let people = client.search_people("Alice").await.unwrap();
    assert_eq!(people.len(), 1);
    let chat = client.create_chat(mock::ME, &people, "").await.unwrap();
    client
        .send_message(
            &chat,
            None,
            &OutgoingMessage::html("test1", "<p>Private message</p>", mock::MY_NAME),
        )
        .await
        .unwrap();
    let messages = client
        .messages(&chat, &MessagesQuery::default())
        .await
        .unwrap()
        .messages;
    assert_eq!(messages[0].content, "<p>Private message</p>");
    let id = messages[0].id.clone();
    client
        .edit_message(
            &chat,
            &id,
            &OutgoingMessage::html("edit1", "<p>Edited</p>", mock::MY_NAME),
        )
        .await
        .unwrap();
    assert_eq!(
        client
            .messages(&chat, &MessagesQuery::default())
            .await
            .unwrap()
            .messages[0]
            .content,
        "<p>Edited</p>"
    );
    client.delete_message(&chat, &id).await.unwrap();
    assert!(
        client
            .messages(&chat, &MessagesQuery::default())
            .await
            .unwrap()
            .messages[0]
            .properties
            .contains_key("deletetime")
    );
    let team = &startup.teams[0];
    let channel = &team.channels[0];
    let posts = client
        .channel_posts(&team.id, &channel.id, None)
        .await
        .unwrap();
    let root = &posts.reply_chains[0].id;
    client
        .send_message(
            &channel.id,
            Some(root),
            &OutgoingMessage::html("reply1", "<p>Thread reply</p>", mock::MY_NAME),
        )
        .await
        .unwrap();
    let replies = client
        .channel_replies(&team.id, &channel.id, root, None)
        .await
        .unwrap();
    assert!(
        replies
            .messages
            .iter()
            .any(|m| m.content == "<p>Thread reply</p>")
    );
    client
        .send_message(
            &channel.id,
            None,
            &OutgoingMessage::html("post1", "<p>New post</p>", mock::MY_NAME),
        )
        .await
        .unwrap();
    assert!(
        client
            .channel_posts(&team.id, &channel.id, None)
            .await
            .unwrap()
            .reply_chains
            .iter()
            .any(|p| p.root().is_some_and(|m| m.content == "<p>New post</p>"))
    );
}
#[test]
fn untitled_groups_use_all_participants_and_never_the_last_sender() {
    let mut c:Conversation=serde_json::from_value(serde_json::json!({"id":"group","properties":{"topic":"  "},"isOneOnOne":false,"members":["me",{"id":"alice","friendlyName":"Alice"},{"mri":"bob"}],"lastMessage":{"imdisplayname":"Last sender"}})).unwrap();
    assert_eq!(chat_title(&c, "me"), "Alice et 1 autre");
    let profiles = HashMap::from([(
        "bob".into(),
        Person {
            mri: "bob".into(),
            display_name: "Bruno".into(),
            ..Default::default()
        },
    )]);
    conversations::resolve_names(&mut c, &profiles);
    assert_eq!(chat_title(&c, "me"), "Alice, Bruno");
    c.members.truncate(2);
    let mut sparse = c.clone();
    sparse.is_one_on_one = None;
    assert!(
        conversations::needs_details(&sparse, "me"),
        "Une liste partielle nommée doit être complétée même sans indicateur de groupe."
    );
    assert_eq!(chat_title(&c, "me"), "Discussion de groupe · Alice");
    c.members.clear();
    assert_eq!(chat_title(&c, "me"), "Discussion de groupe");
    c.properties
        .insert("topic".into(), serde_json::json!("  Projet  "));
    assert_eq!(chat_title(&c, "me"), "Projet");
}
#[tokio::test]
async fn resolves_sparse_roster_profiles_and_downloads_demo_media() {
    let addr = mock::spawn();
    let base = Url::parse(&format!("http://{addr}/")).unwrap();
    let auth = StaticAuthenticator::new()
        .with_service(Service::ChatService, base.clone())
        .with_service(Service::MiddleTier, base.clone())
        .with_service(Service::Csa, base.clone())
        .with_service(Service::Ams, base);
    let client = ChatClient::new(Arc::new(AppAuthenticator::Demo(auth)));
    let startup = client.startup().await.unwrap();
    let c = startup
        .chats
        .iter()
        .find(|c| c.id == "19:projet-rust@thread.v2")
        .unwrap();
    let mut sparse = c.conversation();
    sparse.thread_properties.clear();
    sparse.members.truncate(1);
    let result = conversations::resolve(client.clone(), vec![sparse.clone()], Arc::default()).await;
    assert!(result.warning.is_none(), "{:?}", result.warning);
    conversations::apply_details(
        &mut sparse,
        &conversations::ChatDetails::of(result.chats[0].clone()),
    );
    let profiles = result
        .profiles
        .into_iter()
        .map(|p| (p.mri.clone(), p))
        .collect();
    conversations::resolve_names(&mut sparse, &profiles);
    assert!(
        sparse
            .members
            .iter()
            .any(|m| m.display_name() == Some("Claire Dubois"))
    );
    let loaded = media::load(
        client,
        "https://eu-api.asm.skype.com/v1/objects/demo-image/views/imgo".into(),
    )
    .await
    .unwrap();
    assert_eq!(loaded.bytes, 320 * 160 * 4);
}
#[test]
fn reaction_acknowledgements_update_copies_but_never_another_chat_or_failed_request() {
    let mut app = App::empty();
    *app.me = "me".into();
    let selected = selection("alice");
    *app.selected = Some(selected.clone());
    let mut message = Message {
        id: "42".into(),
        ..Default::default()
    };
    message.apply_reaction("peer", "like", false);
    *app.conv.test_messages() = vec![message.clone()];
    *app.conv.test_replies() = vec![message.clone()];
    let _ = app.update(Msg::from(ConversationMsg::Reacted {
        generation: 0,
        selection: selected.clone(),
        id: "42".into(),
        key: "heart".into(),
        remove: false,
        result: Err("Forbidden".into()),
    }));
    assert_eq!(app.conv.messages()[0].reactions("me").len(), 1);
    assert!(app.error.is_some());
    let _ = app.update(Msg::from(ConversationMsg::Reacted {
        generation: 0,
        selection: selected.clone(),
        id: "42".into(),
        key: "heart".into(),
        remove: false,
        result: Ok(()),
    }));
    assert_eq!(app.conv.messages()[0].reactions("me").len(), 2);
    assert_eq!(app.conv.replies()[0].reactions("me").len(), 2);
    let _ = app.update(Msg::from(ConversationMsg::Reacted {
        generation: 0,
        selection: selection("bob"),
        id: "42".into(),
        key: "heart".into(),
        remove: true,
        result: Ok(()),
    }));
    assert!(
        app.conv.messages()[0]
            .reactions("me")
            .iter()
            .any(|r| r.key == "heart" && r.mine)
    );
    let _ = app.update(Msg::SignOut);
    let _ = app.update(Msg::from(ConversationMsg::Reacted {
        generation: 0,
        selection: selected,
        id: "42".into(),
        key: "heart".into(),
        remove: false,
        result: Ok(()),
    }));
    assert!(app.conv.messages().is_empty());
    assert!(app.error.is_none());
}
#[test]
fn general_channels_and_loaded_channels_survive_partial_startup() {
    let mut team = Team {
        id: "team".into(),
        display_name: "Team".into(),
        ..Default::default()
    };
    normalize_team(&mut team);
    assert_eq!(team.channels.len(), 1);
    assert!(team.channels[0].is_general);
    assert_eq!(team.channels[0].id, "team");
    normalize_team(&mut team);
    assert_eq!(team.channels.len(), 1);
    let mut app = App::empty();
    team.channels.push(Channel {
        id: "private".into(),
        display_name: "Privé".into(),
        channel_type: "Private".into(),
        ..Default::default()
    });
    app.teams.push(team);
    app.channels_loaded.insert("team".into());
    let incoming = workspace(
        serde_json::from_value(
            serde_json::json!({"teams":[{"id":"team","displayName":"Team updated"}]}),
        )
        .unwrap(),
    );
    let _ = app.update(Msg::WorkspaceLoaded(0, Ok(incoming)));
    assert_eq!(app.teams[0].display_name, "Team updated");
    assert!(app.teams[0].channels.iter().any(|c| c.id == "private"));
    let _ = app.update(Msg::ChannelsLoaded {
        generation: 0,
        team: "team".into(),
        result: Ok((
            vec![Channel {
                id: "new".into(),
                display_name: "Nouveau".into(),
                ..Default::default()
            }],
            None,
        )),
    });
    // The endpoint can leave out a private channel the user belongs to: it stays.
    assert!(app.teams[0].channels.iter().any(|c| c.id == "private"));
    // A standard channel the startup listed stays when the endpoint leaves it out.
    app.teams[0].channels.push(Channel {
        id: "listed".into(),
        display_name: "Listé".into(),
        channel_type: "Standard".into(),
        ..Default::default()
    });
    let _ = app.update(Msg::ChannelsLoaded {
        generation: 0,
        team: "team".into(),
        result: Ok((vec![], None)),
    });
    assert!(app.teams[0].channels.iter().any(|c| c.id == "listed"));
    assert!(app.teams[0].channels.iter().any(|c| c.id == "new"));
    *app.selected = Some(Selection::Channel {
        team: "team".into(),
        channel: "new".into(),
    });
    app.teams[0].is_archived = true;
    assert!(app.read_only());
}
#[tokio::test]
async fn demo_reactions_work_on_private_messages_channel_posts_and_replies() {
    let addr = mock::spawn();
    let base = Url::parse(&format!("http://{addr}/")).unwrap();
    let auth = StaticAuthenticator::new()
        .with_service(Service::ChatService, base.clone())
        .with_service(Service::Csa, base.clone())
        .with_service(Service::MiddleTier, base);
    let client = ChatClient::new(Arc::new(auth));
    let teams = client.teams().await.unwrap();
    assert_eq!(teams.len(), 2);
    let team = &teams[0];
    let channels = client.channels(&team.id).await.unwrap();
    assert!(!channels.is_empty());
    let posts = client
        .channel_posts_for(&team.id, &channels[0], None)
        .await
        .unwrap();
    let root = posts.reply_chains[0].root().unwrap();
    for (conv, id) in [(&channels[0].id, &root.id)] {
        client.add_reaction(conv, id, "heart", 100).await.unwrap();
        let messages = client
            .messages(conv, &MessagesQuery::default())
            .await
            .unwrap()
            .messages;
        assert!(
            messages
                .iter()
                .find(|m| &m.id == id)
                .unwrap()
                .reactions(mock::ME)
                .iter()
                .any(|r| r.key == "heart" && r.mine)
        );
        client.add_reaction(conv, id, "laugh", 101).await.unwrap();
        client.remove_reaction(conv, id, "laugh").await.unwrap();
        assert!(
            client
                .messages(conv, &MessagesQuery::default())
                .await
                .unwrap()
                .messages
                .iter()
                .find(|m| &m.id == id)
                .unwrap()
                .reactions(mock::ME)
                .iter()
                .all(|r| !r.mine)
        );
    }
    let private = client.startup().await.unwrap().chats[0].id.clone();
    let message = client
        .messages(&private, &MessagesQuery::default())
        .await
        .unwrap()
        .messages
        .remove(0);
    client
        .add_reaction(&private, &message.id, "like", 200)
        .await
        .unwrap();
    assert!(
        client
            .messages(&private, &MessagesQuery::default())
            .await
            .unwrap()
            .messages
            .iter()
            .find(|m| m.id == message.id)
            .unwrap()
            .reactions(mock::ME)
            .iter()
            .any(|r| r.mine)
    );
    client
        .send_message(
            &channels[0].id,
            Some(&root.id),
            &OutgoingMessage::html("reaction-reply", "Reply", mock::MY_NAME),
        )
        .await
        .unwrap();
    let reply = client
        .channel_replies(&team.id, &channels[0].id, &root.id, None)
        .await
        .unwrap()
        .messages
        .remove(0);
    client
        .add_reaction(&channels[0].id, &reply.id, "heart", 300)
        .await
        .unwrap();
    let replies = client
        .channel_replies(&team.id, &channels[0].id, &root.id, None)
        .await
        .unwrap();
    assert!(
        replies
            .messages
            .iter()
            .find(|m| m.id == reply.id)
            .unwrap()
            .reactions(mock::ME)
            .iter()
            .any(|r| r.mine)
    );
    assert!(
        !client
            .all_thread_members(&channels[0].id)
            .await
            .unwrap()
            .is_empty()
    );
}
#[tokio::test]
async fn teams_load_independently_and_empty_team_endpoint_never_erases_startup_teams() {
    use axum::{Json, Router, routing::get};
    use serde_json::json;
    for (startup_status, startup, teams_status, teams, expected) in [
        (
            200,
            json!({"teams":[{"id":"team","displayName":"Startup team"}]}),
            200,
            json!([]),
            1,
        ),
        (
            200,
            json!({"chats":[{"id":"bad","isRead":"unexpected value"}]}),
            200,
            json!([{"id":"team","displayName":"Dedicated endpoint"}]),
            1,
        ),
        (503, json!({}), 200, json!([{"id":"team"}]), 1),
        (200, json!({"teams":[{"id":"team"}]}), 503, json!({}), 1),
        (503, json!({}), 503, json!({}), 0),
        (200, json!({"teams":[]}), 200, json!([]), 0),
    ] {
        let router = Router::new()
            .route(
                "/api/v3/teams/users/me",
                get(move || async move {
                    (
                        axum::http::StatusCode::from_u16(startup_status).unwrap(),
                        Json(startup),
                    )
                }),
            )
            .route(
                "/api/v1/teams/users/me/teams",
                get(move || async move {
                    (
                        axum::http::StatusCode::from_u16(teams_status).unwrap(),
                        Json(teams),
                    )
                }),
            )
            .route(
                "/v1/users/ME/conversations",
                get(|| async { Json(json!({"conversations":[{"id":"private"}]})) }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let base = Url::parse(&format!("http://{addr}/")).unwrap();
        let auth = StaticAuthenticator::new()
            .with_service(Service::ChatService, base.clone())
            .with_service(Service::Csa, base);
        let client = ChatClient::new(Arc::new(AppAuthenticator::Demo(auth)));
        let result = load_workspace_data(client).await.unwrap();
        assert_eq!(result.teams.len(), expected);
        if startup_status == 503 && teams_status == 503 {
            assert!(result.teams_problem.is_some());
            assert!(!result.teams_complete);
            let mut app = App::empty();
            app.teams.push(Team {
                id: "cached".into(),
                ..Default::default()
            });
            let _ = app.update(Msg::WorkspaceLoaded(0, Ok(result)));
            assert_eq!(app.teams[0].id, "cached");
        } else if startup_status == 200 && teams_status == 200 && expected == 0 {
            assert!(result.teams_problem.is_none());
        }
        server.abort();
    }
}
#[test]
fn numeric_csa_publications_reach_the_feed_and_failed_loads_have_a_retry_state() {
    let mut app = App::empty();
    let selection = Selection::Channel {
        team: "team".into(),
        channel: "channel".into(),
    };
    *app.selected = Some(selection.clone());
    let _ = app.update(Msg::from(ConversationMsg::PostsLoaded {
        generation: 0,
        selection: selection.clone(),
        append: false,
        result: Err("posts field replyChains[0].messages[0].version".into()),
    }));
    assert!(app.conv.posts_error().is_some());
    assert!(app.conv.posts().is_empty());
    let page:PostsPage=serde_json::from_value(serde_json::json!({"replyChains":[{"id":"42","messages":[{"id":"42","version":0,"messageType":"RichText/Html","content":"Publication"}]}]})).unwrap();
    let _ = app.update(Msg::from(ConversationMsg::PostsLoaded {
        generation: 0,
        selection: selection.clone(),
        append: false,
        result: Ok(page),
    }));
    assert!(app.conv.posts_error().is_none());
    assert_eq!(app.conv.posts()[0].root().unwrap().content, "Publication");
    let _ = app.update(Msg::from(ConversationMsg::PostsLoaded {
        generation: 0,
        selection,
        append: false,
        result: Err("Réseau indisponible".into()),
    }));
    assert_eq!(app.conv.posts().len(), 1);
    assert!(app.conv.posts_error().is_some());
}
#[test]
fn participant_warnings_do_not_replace_other_errors_and_refresh_clears_stale_banner() {
    let mut app = App::empty();
    *app.notice = Some("Liste des équipes indisponible".into());
    *app.error = Some("Publications : HTTP 403".into());
    let _ = app.update(Msg::ChatsResolved(
        0,
        conversations::Resolution {
            warning: Some("Participants incomplets".into()),
            ..Default::default()
        },
    ));
    assert_eq!(
        app.notice.as_deref(),
        Some("Liste des équipes indisponible")
    );
    assert_eq!(app.error.as_deref(), Some("Publications : HTTP 403"));
    assert!(app.participants_notice.is_some());
    let _ = app.update(Msg::Refresh);
    assert!(app.participants_notice.is_none());
    let _ = app.update(Msg::ChatsResolved(0, conversations::Resolution::default()));
    assert!(app.participants_notice.is_none());
}
/// Direct chats show their peer's presence by MRI in any case, group chats none; an unchanged poll leaves the sidebar untouched.
#[test]
fn direct_chats_show_their_peer_presence_and_unchanged_polls_redraw_nothing() {
    let mut app = App::empty();
    *app.me = "8:orgid:me".into();
    *app.chats = vec![
        serde_json::from_value(serde_json::json!({
            "id":"direct", "isOneOnOne":true,
            "members":[{"id":"8:orgid:me"},{"id":"8:orgid:Peer"}]
        }))
        .unwrap(),
        serde_json::from_value(serde_json::json!({
            "id":"group", "isOneOnOne":false,
            "members":[{"id":"8:orgid:me"},{"id":"8:orgid:peer"},{"id":"8:orgid:other"}]
        }))
        .unwrap(),
    ];
    let presence = |availability: &str| {
        Msg::PresenceLoaded(
            0,
            Ok(vec![Presence {
                mri: "8:orgid:peer".into(),
                availability: availability.into(),
                activity: None,
                out_of_office: true,
                ..Default::default()
            }]),
        )
    };
    let _ = app.update(presence("AvailableIdle"));
    let rows = app.sidebar().chats;
    let row = |id: &str| rows.iter().find(|r| r.id == id).unwrap().clone();
    assert_eq!(
        row("direct").presence,
        Some(view::PresenceView {
            status: view::PresenceStatus::Away,
            out_of_office: true
        })
    );
    assert_eq!(row("group").presence, None);
    let _ = app.take_changes();
    let _ = app.update(presence("AvailableIdle"));
    assert!(!app.take_changes().has(Parts::SIDEBAR));
}
/// An open group chat asks for the presence of its authors and members, and shows it on their avatars.
#[test]
fn an_open_group_chat_shows_the_presence_of_its_authors_and_members() {
    let mut app = App::empty();
    *app.me = "8:orgid:me".into();
    *app.chats = vec![
        serde_json::from_value(serde_json::json!({
            "id":"a", "isOneOnOne":false,
            "members":[{"id":"8:orgid:me"},{"id":"8:orgid:other"},{"id":"28:app"}]
        }))
        .unwrap(),
    ];
    let _ = app.update(Msg::Select(Selection::Chat("a".into())));
    feed_loaded(&mut app, false, feed_page(0..2, ""));
    assert_eq!(
        app.presence_targets(),
        ["8:orgid:me", "8:orgid:other", "8:orgid:peer"]
    );
    let _ = app.update(Msg::PresenceLoaded(
        0,
        Ok(vec![Presence {
            mri: "8:orgid:peer".into(),
            availability: "Busy".into(),
            ..Default::default()
        }]),
    ));
    let busy = Some(view::PresenceView {
        status: view::PresenceStatus::Busy,
        out_of_office: false,
    });
    assert_eq!(
        app.message_view(&app.conv.messages()[0]).sender_presence,
        busy
    );
    let _ = app.update(Msg::PresenceLoaded(
        0,
        Ok(vec![Presence {
            mri: "8:orgid:other".into(),
            availability: "Busy".into(),
            ..Default::default()
        }]),
    ));
    let members = app.members_view().members.unwrap();
    assert_eq!(members[1].presence, busy);
    assert_eq!(members[2].presence, None);
}
#[test]
fn avatars_show_people_bots_and_direct_chat_peers() {
    let mut app = App::empty();
    *app.me = "8:orgid:me".into();
    let chat = |id: &str, one_on_one: bool, peers: &[&str]| {
        let members: Vec<_> = std::iter::once("8:orgid:me")
            .chain(peers.iter().copied())
            .map(|id| serde_json::json!({ "id": id }))
            .collect();
        serde_json::from_value(serde_json::json!({
            "id": id, "isOneOnOne": one_on_one, "members": members
        }))
        .unwrap()
    };
    *app.chats = vec![
        chat("person", true, &["8:orgid:peer"]),
        chat("bot", true, &["28:app"]),
        chat("group", false, &["8:orgid:peer", "28:app"]),
    ];
    let mri = |id: &str| app.avatar_mri(id);
    assert_eq!(mri("8:orgid:peer").as_deref(), Some("8:orgid:peer"));
    assert_eq!(mri("28:app").as_deref(), Some("28:app"));
    assert_eq!(mri("person").as_deref(), Some("8:orgid:peer"));
    assert_eq!(mri("bot").as_deref(), Some("28:app"));
    assert_eq!(mri("group"), None);
    assert_eq!(mri("19:channel@thread.tacv2"), None);
}
/// As in Teams: apps and teams in a rounded square, people and group chats in a circle.
#[test]
fn apps_and_channels_get_square_avatars_people_and_groups_round_ones() {
    let mut app = App::empty();
    *app.me = "8:orgid:me".into();
    let chat = |id: &str, one_on_one: bool, peers: &[&str]| {
        let members: Vec<_> = std::iter::once("8:orgid:me")
            .chain(peers.iter().copied())
            .map(|id| serde_json::json!({ "id": id }))
            .collect();
        serde_json::from_value(serde_json::json!({
            "id": id, "isOneOnOne": one_on_one, "members": members
        }))
        .unwrap()
    };
    *app.chats = vec![
        chat("person", true, &["8:orgid:peer"]),
        chat("bot", true, &["28:app"]),
        chat("group", false, &["8:orgid:peer", "28:app"]),
    ];
    let rows = app.sidebar().chats;
    let row = |id: &str| rows.iter().find(|r| r.id == id).unwrap().avatar_shape;
    assert_eq!(row("person"), view::AvatarShape::Round);
    assert_eq!(row("bot"), view::AvatarShape::Square);
    assert_eq!(row("group"), view::AvatarShape::Round);

    let header = |app: &App| app.conversation_part().view.unwrap().avatar_shape;
    *app.selected = Some(Selection::Chat("bot".into()));
    assert_eq!(header(&app), view::AvatarShape::Square);
    *app.selected = Some(Selection::Chat("person".into()));
    assert_eq!(header(&app), view::AvatarShape::Round);
    *app.selected = Some(Selection::Channel {
        team: "team".into(),
        channel: "19:c@thread.tacv2".into(),
    });
    assert_eq!(header(&app), view::AvatarShape::Square);

    let from = |mri: &str| Message {
        from: format!("https://x/v1/users/ME/contacts/{mri}"),
        ..Default::default()
    };
    assert_eq!(
        app.message_view(&from("28:app")).sender_avatar_shape,
        view::AvatarShape::Square
    );
    assert_eq!(
        app.message_view(&from("8:orgid:peer")).sender_avatar_shape,
        view::AvatarShape::Round
    );
}

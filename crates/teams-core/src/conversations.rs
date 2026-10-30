//! Hydrate sparse startup rosters and resolve profiles independently of last messages.
use crate::app::*;
use chrono::DateTime;
use teams_i18n::{t, tf};

/// Teams channels also have conversation IDs; never present them as private chats.
pub(crate) fn is_private_chat(chat: &Conversation, teams: &[Team]) -> bool {
    !chat.id.contains("@thread.tacv2")
        && !teams
            .iter()
            .any(|team| team.id == chat.id || team.channels.iter().any(|c| c.id == chat.id))
        && ![&chat.extra, &chat.thread_properties, &chat.properties]
            .into_iter()
            .any(|fields| {
                fields.contains_key("parentTeamId")
                    || fields.contains_key("channelType")
                    || ["threadType", "threadtype", "chatType"]
                        .into_iter()
                        .any(|key| {
                            fields
                                .get(key)
                                .and_then(|v| v.as_str())
                                .is_some_and(|kind| {
                                    ["channel", "team", "space", "community", "subcommunity"]
                                        .iter()
                                        .any(|excluded| kind.eq_ignore_ascii_case(excluded))
                                })
                        })
            })
}

pub(crate) fn message_time(message: &Message) -> Option<i64> {
    [
        message.originalarrivaltime.as_deref(),
        message.composetime.as_deref(),
    ]
    .into_iter()
    .flatten()
    .find_map(|time| {
        DateTime::parse_from_rfc3339(time)
            .ok()
            .map(|t| t.timestamp_millis())
    })
}

pub(crate) fn last_message_time(chat: &Conversation) -> Option<i64> {
    let summary = chat
        .extra
        .get("lastL1Message")
        .and_then(|m| m.get("originalArrivalTime"))
        .and_then(|v| v.as_str())
        .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
        .map(|t| t.timestamp_millis());
    chat.last_message
        .as_ref()
        .and_then(message_time)
        .max(summary)
}

/// A feed or a send may be newer than the next CSA list response.
pub(crate) fn update_last_message(chat: &mut Conversation, message: &Message) {
    let incoming = message_time(message);
    let current = chat.last_message.as_ref();
    if current.is_none()
        || incoming > current.and_then(message_time)
        || current.is_some_and(|m| m.id == message.id)
    {
        chat.last_message = Some(message.clone());
    }
}
#[derive(Debug, Clone, Default)]
pub struct Resolution {
    pub chats: Vec<Conversation>,
    pub profiles: Vec<Person>,
    pub attempted: Vec<String>,
    /// Attempted chats whose roster lookups all failed: worth asking again later.
    pub failed: Vec<String>,
    pub warning: Option<String>,
}
/// What a roster lookup found for a chat (members and properties), reapplied whenever the chat list reloads.
#[derive(Debug, Clone, Default)]
pub struct ChatDetails {
    pub members: Vec<teams_chat::Member>,
    /// Properties, then thread properties, that fill in what the chat list lacks.
    pub properties: Vec<(String, serde_json::Value)>,
    /// `rosterVersion` when it was looked up: a newer one means the roster changed.
    pub roster_version: Option<serde_json::Value>,
}
impl ChatDetails {
    pub fn of(chat: Conversation) -> Self {
        Self {
            roster_version: chat.roster_version.clone(),
            properties: chat
                .properties
                .into_iter()
                .chain(chat.thread_properties)
                .collect(),
            members: chat.members,
        }
    }
}
pub fn apply_details(chat: &mut Conversation, details: &ChatDetails) {
    fill(
        chat,
        &details.members,
        details.properties.iter().map(|(key, value)| (key, value)),
    );
}
/// Applies what a lookup returned for the chat.
fn apply_found(chat: &mut Conversation, found: &Conversation) {
    fill(
        chat,
        &found.members,
        found
            .properties
            .iter()
            .chain(found.thread_properties.iter()),
    );
}
fn fill<'a>(
    chat: &mut Conversation,
    members: &[teams_chat::Member],
    properties: impl Iterator<Item = (&'a String, &'a serde_json::Value)>,
) {
    if !members.is_empty() {
        replace_members(chat, members.to_vec());
    }
    for (key, value) in properties {
        if chat
            .thread_properties
            .get(key)
            .is_none_or(|v| v.as_str().is_none_or(|s| s.trim().is_empty()))
        {
            chat.thread_properties.insert(key.clone(), value.clone());
        }
    }
}
fn replace_members(chat: &mut Conversation, mut members: Vec<teams_chat::Member>) {
    for member in &mut members {
        if let Some(previous) = chat
            .members
            .iter()
            .find(|p| p.id.eq_ignore_ascii_case(&member.id))
        {
            for (key, value) in &previous.extra {
                if member
                    .extra
                    .get(key)
                    .is_none_or(|v| v.is_null() || v.as_str().is_some_and(|s| s.trim().is_empty()))
                {
                    member.extra.insert(key.clone(), value.clone());
                }
            }
            if member.role.is_none() {
                member.role = previous.role.clone();
            }
        }
    }
    chat.members = members;
}
fn describe(error: &teams_chat::Error) -> String {
    use teams_chat::Error;
    match error {
        Error::RateLimited { retry_after_secs } => {
            tf!(
                "core.conversations.teams_is_limiting_requests_http_429",
                retry_after_secs = retry_after_secs
            )
        }
        Error::Status {
            endpoint, status, ..
        } => tf!(
            "core.conversations.http",
            endpoint = endpoint,
            status = status
        ),
        Error::DecodeField { endpoint, path, .. } => {
            tf!(
                "core.conversations.incompatible_format_at_field",
                endpoint = endpoint,
                path = path
            )
        }
        Error::Decode { endpoint, .. } => tf!(
            "core.conversations.incompatible_json_format",
            endpoint = endpoint
        ),
        Error::Auth(_) => {
            t("core.conversations.participants_authentication_or_paging_refused").into()
        }
        Error::Http(e) if e.is_timeout() => {
            t("core.conversations.loading_the_participants_timed_out").into()
        }
        _ => t("core.conversations.cannot_reach_the_participants_service").into(),
    }
}
pub fn resolve_names(chat: &mut Conversation, profiles: &HashMap<String, Person>) {
    resolve_names_in(chat, profiles, &mut None);
}
/// Profiles by lowercase MRI, built on the first miss and shared by every chat of one pass.
pub(crate) type CaseIndex<'a> = Option<HashMap<String, &'a Person>>;
pub(crate) fn resolve_names_in<'a>(
    chat: &mut Conversation,
    profiles: &'a HashMap<String, Person>,
    index: &mut CaseIndex<'a>,
) {
    for member in &mut chat.members {
        // The middle tier may answer with another case than the roster's MRI.
        let person = profiles.get(&member.id).or_else(|| {
            index
                .get_or_insert_with(|| {
                    let mut index = HashMap::with_capacity(profiles.len());
                    for p in profiles.values() {
                        index.entry(p.mri.to_ascii_lowercase()).or_insert(p);
                    }
                    index
                })
                .get(&member.id.to_ascii_lowercase())
                .copied()
        });
        if let Some(person) = person
            && let Some(name) = person.known_name()
        {
            set_name(member, name);
        }
    }
}
pub fn needs_details(chat: &Conversation, me: &str) -> bool {
    let complete_dm = chat.is_one_on_one()
        && chat.members.len() >= 2
        && chat
            .members
            .iter()
            .all(|m| m.id.eq_ignore_ascii_case(me) || m.display_name().is_some());
    chat.needs_thread_sync()
        || !complete_dm
            && (chat.title().is_none()
                || chat.is_group()
                || chat.members.is_empty()
                || chat
                    .members
                    .iter()
                    .any(|m| !m.id.eq_ignore_ascii_case(me) && m.display_name().is_none()))
}
/// `imdisplayname`, unless empty or just a placeholder for the sender's MRI.
fn author_name(message: &Message) -> Option<&str> {
    let mri = message.sender_mri();
    message
        .imdisplayname
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty() && !teams_chat::is_placeholder_name(mri, name))
}
fn set_name(member: &mut teams_chat::Member, name: &str) {
    member
        .extra
        .insert("friendlyName".into(), serde_json::json!(name));
}
/// Names nameless members from their messages: an external peer no directory answers for still signs with `imdisplayname`.
pub(crate) fn name_members_from_messages<'a>(
    chat: &mut Conversation,
    messages: impl IntoIterator<Item = &'a Message>,
) {
    let mut nameless: Vec<_> = chat
        .members
        .iter_mut()
        .filter(|m| m.display_name().is_none())
        .collect();
    if nameless.is_empty() {
        return;
    }
    for message in messages {
        // A bot signs with its app's name, as the conversation shows it.
        let Some(name) = crate::app::view::app_metadata_name(message)
            .or_else(|| author_name(message).map(str::to_owned))
        else {
            continue;
        };
        let mri = message.sender_mri();
        if let Some(index) = nameless.iter().position(|m| m.id.eq_ignore_ascii_case(mri)) {
            set_name(nameless.swap_remove(index), &name);
        }
    }
}
/// The two AAD object ids in `19:[{cloud}:]{a}_{b}@unq.gbl.spaces` (cloud prefix in sovereign clouds).
fn direct_chat_ids(chat: &Conversation) -> Option<[&str; 2]> {
    let ids = chat
        .id
        .strip_prefix("19:")?
        .strip_suffix("@unq.gbl.spaces")?;
    let ids = ids.rsplit_once(':').map_or(ids, |(_, ids)| ids);
    let (first, second) = ids.split_once('_')?;
    let guid = |id: &str| uuid::Uuid::parse_str(id).is_ok();
    (guid(first) && guid(second)).then_some([first, second])
}
/// A 1:1 chat, also when the chat list omits `isOneOnOne` (federated chats).
fn is_direct(chat: &Conversation) -> bool {
    chat.is_one_on_one() || (chat.is_one_on_one.is_none() && direct_chat_ids(chat).is_some())
}
fn usable_roster(chat: &Conversation) -> bool {
    if is_direct(chat) {
        chat.members.len() >= 2
    } else {
        !chat.members.is_empty()
    }
}
/// Rebuilds a direct chat's roster from its id (CSA may omit the external member); only those ids' authors lend a name.
fn recover_direct_members(chat: &mut Conversation, messages: &[Message]) {
    if !is_direct(chat) {
        return;
    }
    let Some(ids) = direct_chat_ids(chat) else {
        return;
    };
    let ids = ids.map(str::to_owned);
    for message in messages.iter().rev() {
        let mri = message.sender_mri();
        let Some(id) = mri.strip_prefix("8:orgid:") else {
            continue;
        };
        if !ids
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(id))
        {
            continue;
        }
        let name = author_name(message);
        if let Some(member) = chat
            .members
            .iter_mut()
            .find(|m| m.id.eq_ignore_ascii_case(mri))
        {
            if member.display_name().is_none()
                && let Some(name) = name
            {
                set_name(member, name);
            }
        } else {
            let mut member = teams_chat::Member {
                id: mri.to_owned(),
                ..Default::default()
            };
            if let Some(name) = name {
                set_name(&mut member, name);
            }
            chat.members.push(member);
        }
    }
    for id in ids {
        let mri = format!("8:orgid:{id}");
        if !chat.members.iter().any(|m| m.id.eq_ignore_ascii_case(&mri)) {
            chat.members.push(teams_chat::Member {
                id: mri,
                ..Default::default()
            });
        }
    }
}
pub async fn resolve(
    client: Client,
    chats: Vec<Conversation>,
    known: Arc<HashMap<String, Person>>,
) -> Resolution {
    let mut result = Resolution::default();
    let mut tasks = tokio::task::JoinSet::new();
    let mut pending = chats.into_iter();
    let mut failures = 0;
    let mut reasons = Vec::new();
    loop {
        while tasks.len() < 2 {
            let Some(chat) = pending.next() else {
                break;
            };
            let client = client.clone();
            tasks.spawn(async move {
                let mut details = chat.clone();
                let mut success = false;
                let mut errors = Vec::new();
                // Prefer CSA, the chat list's source; IC3 thread/member requests only when its roster is unusable.
                match client.chat(&chat.id).await {
                    Ok(full) => {
                        let full = full.conversation();
                        if !full.members.is_empty() {
                            apply_found(&mut details, &full);
                            success = usable_roster(&details);
                        }
                    }
                    Err(e) => errors.push(describe(&e)),
                }
                if !success {
                    match client.thread(&chat.id).await {
                        Ok(thread) => {
                            apply_found(&mut details, &thread);
                            success = usable_roster(&details);
                        }
                        Err(e) => errors.push(describe(&e)),
                    }
                    match client.all_thread_members(&chat.id).await {
                        Ok(members) => {
                            if !members.is_empty() {
                                replace_members(&mut details, members);
                                success = usable_roster(&details);
                            }
                        }
                        Err(e) => errors.push(describe(&e)),
                    }
                }
                if !usable_roster(&details) && is_direct(&details) {
                    match client
                        .messages(
                            &chat.id,
                            &MessagesQuery {
                                page_size: Some(50),
                                ..Default::default()
                            },
                        )
                        .await
                    {
                        Ok(page) => {
                            recover_direct_members(&mut details, &page.messages);
                            success = usable_roster(&details);
                        }
                        Err(error) => {
                            // The chat id still names the peer, for its profile lookup.
                            recover_direct_members(&mut details, &[]);
                            success = usable_roster(&details);
                            errors.push(describe(&error));
                        }
                    }
                }
                (chat.id, success, details, errors)
            });
        }
        let Some(item) = tasks.join_next().await else {
            break;
        };
        if let Ok((id, success, chat, errors)) = item {
            result.attempted.push(id.clone());
            result.chats.push(chat);
            if !success {
                result.failed.push(id);
                failures += 1;
                for error in errors {
                    if !reasons.contains(&error) {
                        reasons.push(error);
                    }
                }
            }
        }
    }
    if failures > 0 {
        let reasons = if reasons.is_empty() {
            t("core.conversations.the_service_returned_no_participants").into()
        } else {
            reasons.join(t("core.conversations.text"))
        };
        result.warning = Some(if failures == 1 {
            tf!(
                "core.conversations.incomplete_participants_for_1_chat_names",
                reasons = reasons
            )
        } else {
            tf!(
                "core.conversations.incomplete_participants_for_chats_names_already",
                failures = failures,
                reasons = reasons
            )
        });
    }
    let mut mris: Vec<_> = result
        .chats
        .iter()
        .flat_map(|c| c.members.iter())
        .filter(|m| {
            !m.id.is_empty()
                && m.display_name().is_none()
                && known.get(&m.id).is_none_or(|p| p.known_name().is_none())
        })
        .map(|m| m.id.clone())
        .collect();
    mris.sort();
    mris.dedup();
    for batch in mris.chunks(100) {
        match client.people_profiles(batch).await {
            Ok(profiles) => result.profiles.extend(profiles),
            Err(error) => {
                let message = tf!(
                    "core.conversations.some_names_are_not_available_in",
                    error = describe(&error)
                );
                if let Some(warning) = &mut result.warning {
                    warning.push(' ');
                    warning.push_str(&message);
                } else {
                    result.warning = Some(message);
                }
            }
        }
    }
    // A peer no directory names: its messages still do, as the conversation shows them.
    let unnamed: Vec<_> = result
        .chats
        .iter()
        .enumerate()
        .filter(|(_, chat)| {
            is_direct(chat)
                && chat.members.iter().any(|m| {
                    m.display_name().is_none()
                        && known.get(&m.id).is_none_or(|p| p.known_name().is_none())
                        && !result
                            .profiles
                            .iter()
                            .any(|p| p.mri.eq_ignore_ascii_case(&m.id) && p.known_name().is_some())
                })
        })
        .map(|(index, chat)| (index, chat.id.clone()))
        .collect();
    for (index, id) in unnamed {
        let query = MessagesQuery {
            page_size: Some(50),
            ..Default::default()
        };
        if let Ok(page) = client.messages(&id, &query).await {
            name_members_from_messages(&mut result.chats[index], &page.messages);
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    const ME: &str = "6763e6ac-1bfd-48ec-8352-a3e3a066c81c";
    const PEER: &str = "c9510018-f17f-48f0-a836-4251022f00f7";
    #[test]
    fn silent_external_peer_is_recovered_from_the_chat_id_even_without_one_on_one_flag() {
        for id in [
            format!("19:{ME}_{PEER}@unq.gbl.spaces"),
            format!("19:gcch:{PEER}_{ME}@unq.gbl.spaces"),
        ] {
            let mut chat: Conversation = serde_json::from_value(json!({
                "id": id, "members": [{"id": format!("8:orgid:{ME}")}]
            }))
            .unwrap();
            assert!(!usable_roster(&chat));
            recover_direct_members(&mut chat, &[]);
            assert_eq!(chat.members.len(), 2);
            assert!(usable_roster(&chat));
            let profiles = HashMap::from([(
                format!("8:orgid:{}", PEER.to_uppercase()),
                Person {
                    mri: format!("8:orgid:{}", PEER.to_uppercase()),
                    display_name: "Bruno Externe".into(),
                    ..Default::default()
                },
            )]);
            resolve_names(&mut chat, &profiles);
            assert_eq!(chat_title(&chat, &format!("8:orgid:{ME}")), "Bruno Externe");
            assert!(!chat.is_group());
        }
        let group: Conversation = serde_json::from_value(json!({
            "id": "19:abc@thread.v2", "members": [{"id": "8:orgid:me"}]
        }))
        .unwrap();
        assert!(!is_direct(&group));
    }
    #[test]
    fn nameless_members_take_the_name_their_messages_carry() {
        let mut chat: Conversation = serde_json::from_value(json!({
            "id": "chat", "isOneOnOne": true,
            "members": [{"id": "8:orgid:me", "friendlyName": "Moi"}, {"id": "8:orgid:Peer"}]
        }))
        .unwrap();
        let messages: Vec<Message> = serde_json::from_value(json!([
            {"from": "8:orgid:stranger", "imdisplayname": "Autre personne"},
            {"from": "8:orgid:me", "imdisplayname": "Pas moi"},
            {"from": "8:orgid:peer", "imdisplayname": "8:orgid:peer"},
            {"from": "8:orgid:peer", "imdisplayname": "Maxime Externe"}
        ]))
        .unwrap();
        name_members_from_messages(&mut chat, &messages);
        assert_eq!(chat.members.len(), 2);
        assert_eq!(chat.members[0].display_name(), Some("Moi"));
        assert_eq!(chat_title(&chat, "8:orgid:me"), "Maxime Externe");
    }
    #[test]
    fn a_bot_is_named_after_its_app_not_its_placeholder() {
        const BOT: &str = "ca9e26b7-dce5-44a0-b2b7-a70a3d65ce25";
        let mut chat: Conversation = serde_json::from_value(json!({
            "id": "chat", "isOneOnOne": true,
            "members": [{"id": "8:orgid:me"}, {"id": format!("28:{BOT}"), "friendlyName": format!("Bot-{BOT}")}]
        }))
        .unwrap();
        assert_eq!(chat.members[1].display_name(), None);
        let messages: Vec<Message> = serde_json::from_value(json!([
            {"from": format!("28:{BOT}"), "imdisplayname": format!("Bot-{}", BOT.to_uppercase())},
            {"from": format!("28:{BOT}"), "imdisplayname": "Bot-x",
             "properties": {"fromAppMetadata": "{\"id\":\"app\",\"name\":\"GitHub\"}"}}
        ]))
        .unwrap();
        name_members_from_messages(&mut chat, &messages);
        assert_eq!(chat_title(&chat, "8:orgid:me"), "GitHub");
        let placeholder = Person {
            mri: format!("28:{BOT}"),
            display_name: format!("Bot-{BOT}"),
            ..Default::default()
        };
        assert_eq!(placeholder.known_name(), None);
        assert!(!teams_chat::is_placeholder_name("28:other", &format!("Bot-{BOT}")));
    }
    #[test]
    fn legacy_direct_chat_recovers_only_authors_identified_by_its_id() {
        let mut chat: Conversation = serde_json::from_value(json!({
            "id":format!("19:{ME}_{PEER}@unq.gbl.spaces"),"isOneOnOne":true,
            "members":[{"id":format!("8:orgid:{ME}")}]
        }))
        .unwrap();
        let messages: Vec<Message> = serde_json::from_value(json!([
            {"from":format!("8:orgid:{PEER}"),"imdisplayname":"Bruno Externe"},
            {"from":"19:system","imdisplayname":"Réunion"},
            {"from":"8:orgid:stranger","imdisplayname":"Autre personne"},
            {"from":format!("8:orgid:{ME}"),"imdisplayname":"Moi"}
        ]))
        .unwrap();
        recover_direct_members(&mut chat, &messages);
        assert_eq!(chat.members.len(), 2);
        assert_eq!(
            chat.members
                .iter()
                .find(|m| m.id == format!("8:orgid:{PEER}"))
                .unwrap()
                .display_name(),
            Some("Bruno Externe")
        );
        chat.is_one_on_one = Some(false);
        chat.members.truncate(1);
        recover_direct_members(&mut chat, &messages);
        assert_eq!(chat.members.len(), 1);
    }
    #[tokio::test]
    async fn self_only_csa_roster_fetches_missing_external_peer() {
        use axum::{
            Json, Router,
            routing::{get, post},
        };
        let router = Router::new()
            .route(
                "/api/v1/teams/users/me/chats/chat",
                get(|| async {
                    Json(json!({
                        "id":"chat", "isOneOnOne":true, "members":[{"mri":"me"}]
                    }))
                }),
            )
            .route(
                "/v1/threads/chat",
                get(|| async {
                    Json(json!({
                        "id":"chat", "members":[{"id":"me"},{"id":"external"}]
                    }))
                }),
            )
            .route(
                "/v1/threads/chat/members",
                get(|| async {
                    Json(json!({
                        "members":[{"id":"me"},{"id":"external"}]
                    }))
                }),
            )
            .route(
                "/beta/users/fetchShortProfile",
                post(|| async { Json(json!({"value":[]})) }),
            )
            .route(
                "/beta/users/fetchFederated",
                post(|| async {
                    Json(json!({"value":[
                        {"mri":"external","displayName":"Bruno Externe"}
                    ]}))
                }),
            );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let auth = StaticAuthenticator::new()
            .with_service(Service::Csa, base.clone())
            .with_service(Service::ChatService, base.clone())
            .with_service(Service::MiddleTier, base);
        let client = ChatClient::new(Arc::new(AppAuthenticator::Demo(auth)));
        let chat: Conversation = serde_json::from_value(json!({"id":"chat","isOneOnOne":true,
            "members":[{"id":"me"}]}))
        .unwrap();
        let mut result = resolve(
            client,
            vec![chat],
            Arc::new(HashMap::from([(
                "me".into(),
                Person {
                    mri: "me".into(),
                    display_name: "Moi".into(),
                    ..Default::default()
                },
            )])),
        )
        .await;
        assert!(result.warning.is_none(), "{:?}", result.warning);
        assert_eq!(result.chats[0].members.len(), 2);
        let profiles = result
            .profiles
            .into_iter()
            .map(|p| (p.mri.clone(), p))
            .collect();
        resolve_names(&mut result.chats[0], &profiles);
        assert_eq!(
            result.chats[0].members[1].display_name(),
            Some("Bruno Externe")
        );
        server.abort();
    }
    #[test]
    fn known_direct_messages_need_no_roster_request_and_member_names_survive_refresh() {
        let mut chat:Conversation=serde_json::from_value(json!({"id":"chat","isOneOnOne":true,"members":[{"id":"me"},{"id":"peer","friendlyName":"Alice","role":"User"}]})).unwrap();
        assert!(!needs_details(&chat, "me"));
        let details: Conversation = serde_json::from_value(
            json!({"id":"chat","members":[{"id":"me"},{"id":"peer","friendlyName":null}]}),
        )
        .unwrap();
        apply_details(&mut chat, &ChatDetails::of(details.clone()));
        assert_eq!(chat.members[1].display_name(), Some("Alice"));
        assert_eq!(chat.members[1].role.as_deref(), Some("User"));
        chat.needs_thread_sync = Some(true);
        assert!(needs_details(&chat, "me"));
    }
    #[tokio::test]
    async fn csa_chat_recovers_rosters_and_failures_keep_names_with_safe_diagnostics() {
        use axum::{
            Json, Router,
            routing::{get, post},
        };
        for csa_success in [true, false] {
            let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
            let thread_calls = calls.clone();
            let member_calls = calls.clone();
            let router=Router::new()
                .route("/v1/threads/chat",get(move||{thread_calls.fetch_add(1,std::sync::atomic::Ordering::SeqCst);async{(axum::http::StatusCode::FORBIDDEN,"private response body")}}))
                .route("/v1/threads/chat/members",get(move||{member_calls.fetch_add(1,std::sync::atomic::Ordering::SeqCst);async{axum::http::StatusCode::NOT_FOUND}}))
                .route("/api/v1/teams/users/me/chats/chat",get(move||async move{
                    if csa_success{(axum::http::StatusCode::OK,Json(json!({"id":"chat","members":[{"mri":"peer","friendlyName":"Alice"},{"mri":"second","friendlyName":"Bruno"}]})))}
                    else{(axum::http::StatusCode::FORBIDDEN,Json(json!({"private":"secret"})))}
                }))
                .route("/beta/users/fetchShortProfile",post(||async{panic!("Already-known names must not trigger profile lookup");#[allow(unreachable_code)] Json(json!({"value":[]}))}));
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
            let base = Url::parse(&format!("http://{addr}/")).unwrap();
            let auth = StaticAuthenticator::new()
                .with_service(Service::ChatService, base.clone())
                .with_service(Service::Csa, base.clone())
                .with_service(Service::MiddleTier, base);
            let client = ChatClient::new(Arc::new(AppAuthenticator::Demo(auth)));
            let chat:Conversation=serde_json::from_value(json!({"id":"chat","isOneOnOne":false,"members":[{"id":"peer","friendlyName":"Alice"}]})).unwrap();
            let result = resolve(client, vec![chat], Arc::default()).await;
            assert_eq!(
                calls.load(std::sync::atomic::Ordering::SeqCst),
                if csa_success { 0 } else { 2 }
            );
            assert_eq!(result.chats[0].members[0].display_name(), Some("Alice"));
            if csa_success {
                assert!(result.warning.is_none(), "{:?}", result.warning);
                assert_eq!(result.chats[0].members.len(), 2);
            } else {
                let warning = result.warning.unwrap();
                assert!(warning.contains("403 Forbidden"));
                assert!(warning.contains("404 Not Found"));
                assert!(!warning.contains("private response body"));
                assert!(!warning.contains("secret"));
            }
            server.abort();
        }
    }
}

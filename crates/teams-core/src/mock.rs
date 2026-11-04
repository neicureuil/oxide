//! In-process fake chat service serving the real routes, so the UI exercises the real `ChatClient` over HTTP; data is fictitious.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::http::{HeaderMap, HeaderValue};
use axum::routing::get;
use axum::routing::{post, put};
use axum::{Json, Router};
use chrono::{SecondsFormat, Utc};
use serde_json::{Value, json};
use teams_chat::{Conversation, Member, Message};

pub const ME: &str = "8:orgid:00000000-0000-0000-0000-000000000001";
pub const MY_NAME: &str = "Vous";
const REPLY_DELAY: Duration = Duration::from_millis(1500);

pub(crate) struct Chat {
    conversation: Conversation,
    peer: (&'static str, &'static str),
    messages: Vec<Message>,
}

pub(crate) type Db = Arc<Mutex<Vec<Chat>>>;

/// Starts the fake service on a background runtime and returns its address.
pub fn spawn() -> SocketAddr {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("mock runtime");
        rt.block_on(async move {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("bind mock");
            tx.send(listener.local_addr().expect("mock addr"))
                .expect("send addr");
            axum::serve(listener, router(seed()))
                .await
                .expect("mock server");
        });
    });
    rx.recv().expect("mock server started")
}

fn router(db: Db) -> Router {
    Router::new()
        .route("/v1/users/ME/conversations", get(list_conversations))
        .route(
            "/v1/users/ME/conversations/{conv}/messages",
            get(list_messages).post(post_message),
        )
        .route("/api/v3/teams/users/me", get(startup))
        .route(
            "/api/v1/teams/users/me/teams",
            get(|headers: HeaderMap| async move {
                Json(teams_data(&crate::files_mock::base(&headers)))
            }),
        )
        .route("/api/v1/teams/{team}/channels/{channel}", get(posts))
        .route("/api/v3/teams/users/me/chats", get(csa_chats))
        .route(
            "/api/v1/teams/users/me/teams/{team}/channels",
            get(team_channels),
        )
        .route("/api/v2/teams/{team}/channels/{channel}", get(posts))
        .route(
            "/api/v1/teams/{team}/channels/{channel}/posts/{post}/replies",
            get(replies),
        )
        .route("/beta/users/searchV2", post(people))
        .route("/beta/users/fetchShortProfile", post(profiles))
        .route("/v1/presence/getpresence/", post(presence))
        .route("/v1/threads/{conv}", get(thread))
        .route("/v1/threads/{conv}/consumptionhorizons", get(horizons))
        .route(
            "/v1/threads/{conv}/members",
            get(members).put(|| async { Json(json!({})) }),
        )
        .route(
            "/v1/threads/{conv}/members/{mri}",
            axum::routing::delete(|| async { StatusCode::OK }),
        )
        .route(
            "/v1/threads/{conv}/properties",
            put(|| async { StatusCode::OK }),
        )
        .route("/searchservice/api/v2/query", post(search))
        .route("/v1/objects/demo-image/views/imgo", get(demo_image))
        .route(
            "/v1/objects/",
            post(|| async { (StatusCode::CREATED, Json(json!({"id": "demo-pasted"}))) }),
        )
        .route(
            "/v1/objects/{id}/content/imgpsh",
            put(|| async { StatusCode::CREATED }),
        )
        .route("/v1/url/info", get(url_info))
        .route("/v1/threads", post(create_chat))
        .route(
            "/v1/users/ME/conversations/{conv}/messages/{id}/properties",
            put(add_reaction).delete(remove_reaction),
        )
        .route(
            "/v1/users/ME/conversations/{conv}/messages/{id}",
            put(edit_message).delete(delete_message),
        )
        .with_state(db.clone())
        .merge(crate::notifications_mock::router(db))
        .merge(crate::calendar_mock::router())
        .merge(crate::calls_mock::router())
        .merge(crate::files_mock::router())
        .merge(crate::manage_mock::router())
}

async fn list_conversations(State(db): State<Db>) -> Json<Value> {
    let mut chats: Vec<Conversation> = db
        .lock()
        .unwrap()
        .iter()
        .map(|c| Conversation {
            last_message: c.messages.last().cloned(),
            ..c.conversation.clone()
        })
        .collect();
    chats.sort_by(|a, b| last_time(b).cmp(last_time(a)));
    Json(json!({ "conversations": chats, "_metadata": {} }))
}

/// Newest first, like the real service.
async fn list_messages(
    State(db): State<Db>,
    Path(conv): Path<String>,
    headers: HeaderMap,
) -> Result<Json<Value>, StatusCode> {
    let (id, reply) = conv
        .split_once(";messageid=")
        .map(|(a, b)| (a, Some(b)))
        .unwrap_or((&conv, None));
    let db = db.lock().unwrap();
    let chat = db
        .iter()
        .find(|c| c.conversation.id == id)
        .ok_or(StatusCode::NOT_FOUND)?;
    let messages: Vec<&Message> = chat
        .messages
        .iter()
        .filter(|m| reply.is_none() || m.extra.get("replyChainId").and_then(Value::as_str) == reply)
        .rev()
        .collect();
    // Demo attachments point at the fake OneDrive, whose address is only known here.
    let body = json!({ "messages": messages, "_metadata": {} })
        .to_string()
        .replace("{BASE}", &crate::files_mock::base(&headers));
    Ok(Json(serde_json::from_str(&body).unwrap_or_default()))
}

async fn post_message(
    State(db): State<Db>,
    Path(conv): Path<String>,
    Json(body): Json<Value>,
) -> Result<(StatusCode, Json<Value>), StatusCode> {
    let (base, reply) = conv
        .split_once(";messageid=")
        .map(|(a, b)| (a.to_owned(), Some(b.to_owned())))
        .unwrap_or((conv.clone(), None));
    let conv = base;
    let now = now_str();
    // Typing indicators are not messages: nobody stores or answers them.
    if body["messagetype"]
        .as_str()
        .is_some_and(|t| t.starts_with("Control/"))
    {
        return Ok((
            StatusCode::CREATED,
            Json(json!({ "OriginalArrivalTime": now })),
        ));
    }
    let content = body["content"].as_str().unwrap_or_default().to_owned();
    let peer = {
        let mut db = db.lock().unwrap();
        let chat = db
            .iter_mut()
            .find(|c| c.conversation.id == conv)
            .ok_or(StatusCode::NOT_FOUND)?;
        let mut msg = message(&conv, ME, MY_NAME, &content, &now);
        msg.clientmessageid = body["clientmessageid"].as_str().map(str::to_owned);
        if let Some(props) = body["properties"].as_object() {
            msg.properties = props.clone();
        }
        if let Some(reply) = &reply {
            msg.extra.insert("replyChainId".into(), json!(reply));
        }
        chat.messages.push(msg);
        chat.peer
    };

    let db = db.clone();
    tokio::spawn(async move {
        // The peer reads, types for a moment, then answers.
        tokio::time::sleep(REPLY_DELAY / 3).await;
        if !conv.contains("tacv2") {
            crate::notifications_mock::push_typing(&conv, peer.0, peer.1);
        }
        tokio::time::sleep(REPLY_DELAY * 2 / 3).await;
        let reply = format!("<p>Bien reçu : {}</p>", strip_p(&content));
        let mut db = db.lock().unwrap();
        if let Some(chat) = db.iter_mut().find(|c| c.conversation.id == conv) {
            let mut message = message(&conv, peer.0, peer.1, &reply, &now_str());
            if conv.contains("tacv2")
                && let Some(last) = chat.messages.last()
            {
                let root = last
                    .extra
                    .get("replyChainId")
                    .and_then(Value::as_str)
                    .unwrap_or(&last.id)
                    .to_owned();
                message.extra.insert("replyChainId".into(), json!(root));
                if let Some(link) = &mut message.conversation_link {
                    link.push_str(&format!(";messageid={root}"));
                }
            }
            crate::notifications_mock::push_new_message(&conv, &message);
            if !conv.contains("tacv2") {
                crate::notifications_mock::push_horizon(&conv, peer.0, &message);
            }
            chat.messages.push(message);
        }
    });

    Ok((
        StatusCode::CREATED,
        Json(json!({ "OriginalArrivalTime": now })),
    ))
}

fn last_time(c: &Conversation) -> &str {
    c.last_message
        .as_ref()
        .and_then(|m| m.composetime.as_deref())
        .unwrap_or_default()
}

fn strip_p(html: &str) -> &str {
    html.trim()
        .trim_start_matches("<p>")
        .trim_end_matches("</p>")
}

fn now_str() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn minutes_ago(m: i64) -> String {
    (Utc::now() - chrono::Duration::minutes(m)).to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn message(conv: &str, mri: &str, name: &str, html: &str, time: &str) -> Message {
    let id = chrono::DateTime::parse_from_rfc3339(time)
        .map(|t| t.timestamp_millis().to_string())
        .unwrap_or_default();
    Message {
        id: id.clone(),
        version: Some(id),
        messagetype: "RichText/Html".into(),
        contenttype: Some("text".into()),
        content: html.into(),
        from: format!("https://mock/v1/users/ME/contacts/{mri}"),
        imdisplayname: Some(name.into()),
        composetime: Some(time.into()),
        originalarrivaltime: Some(time.into()),
        conversation_link: Some(format!("https://mock/v1/users/ME/conversations/{conv}")),
        ..Default::default()
    }
}

/// Seed message: minutes ago, sent by me, HTML body.
type SeedMessage = (i64, bool, &'static str);
/// Seed chat: id, topic (empty for 1:1), peer (MRI, name), messages.
type SeedChat = (
    &'static str,
    &'static str,
    (&'static str, &'static str),
    &'static [SeedMessage],
);

fn seed() -> Db {
    let specs: [SeedChat; 4] = [
        (
            "19:alice_me@unq.gbl.spaces",
            "",
            ("8:orgid:alice", "Alice Martin"),
            &[
                (
                    180,
                    false,
                    "<p>Salut ! Tu as vu la démo du nouveau client ?</p>",
                ),
                (175, true, "<p>Oui, ça tourne en <b>Rust</b> en natif.</p>"),
                (170, false, "<p>Combien de RAM ?</p>"),
                (165, true, "<p>Moins de 100&nbsp;Mo pour l'instant 🙂</p>"),
            ],
        ),
        (
            "19:projet-rust@thread.v2",
            "Projet client Rust",
            ("8:orgid:bob", "Bruno Lefèvre"),
            &[
                (
                    90,
                    false,
                    "<p>Le client chat service est typé, les tests passent.</p>",
                ),
                (60, true, "<p>Top, je branche l'UI dessus.</p>"),
                // Rich bodies as Teams sends them: reply quote, emoji, mention, code, table.
                (
                    50,
                    false,
                    r#"<blockquote itemscope itemtype="http://schema.skype.com/Reply" itemid="1"><strong itemprop="mri" itemid="8:orgid:me">Vous</strong><span itemprop="time" itemid="1700000000000"></span><p itemprop="preview">Top, je branche l'UI dessus.</p></blockquote><p>Super <span class="animated-emoticon-20-clap"><img itemscope itemtype="http://schema.skype.com/Emoji" itemid="clap" src="https://statics.teams.cdn.office.net/evergreen-assets/personal-expressions/v2/assets/emoticons/clap/default/20_f.png" alt="👏" width="20" height="20"></span> <span itemscope itemtype="http://schema.skype.com/Mention" itemid="0">Claire</span>, voici l'exemple :</p><pre class="language-rust"><code>fn main() {
    let client = Client::new(auth);
    client.run().await?;
}</code></pre><ol><li><p>Typer le client</p></li><li>Brancher l'UI<ul><li>liste des chats</li><li>messages</li></ul></li></ol><table><tbody><tr><th>Crate</th><th>État</th></tr><tr><td>teams-chat</td><td><b>OK</b></td></tr><tr><td>teams-calling</td><td>en cours</td></tr></tbody></table>"#,
                ),
                (
                    40,
                    false,
                    "<URIObject type=\"SWIFT.1\" uri=\"\"><Swift b64=\"eyJ0eXBlIjoibWVzc2FnZS9jYXJkIiwiYXR0YWNobWVudHMiOlt7ImNvbnRlbnRUeXBlIjoiYXBwbGljYXRpb24vdm5kLm1pY3Jvc29mdC5jYXJkLmFkYXB0aXZlIiwiY29udGVudCI6eyJ0eXBlIjoiQWRhcHRpdmVDYXJkIiwidmVyc2lvbiI6IjEuNCIsImJvZHkiOlt7InR5cGUiOiJUZXh0QmxvY2siLCJ0ZXh0IjoiQnVpbGQgIzEyOCByw6l1c3NpIiwid2VpZ2h0IjoiQm9sZGVyIiwic2l6ZSI6Ik1lZGl1bSJ9LHsidHlwZSI6IlRleHRCbG9jayIsInRleHQiOiJCcmFuY2hlICoqbWFpbioqIMK3IDQgbWluIDEyIHMiLCJpc1N1YnRsZSI6dHJ1ZX0seyJ0eXBlIjoiRmFjdFNldCIsImZhY3RzIjpbeyJ0aXRsZSI6IlRlc3RzIiwidmFsdWUiOiIzMTIgLyAzMTIifSx7InRpdGxlIjoiQ291dmVydHVyZSIsInZhbHVlIjoiODEgJSJ9XX1dLCJhY3Rpb25zIjpbeyJ0eXBlIjoiQWN0aW9uLk9wZW5VcmwiLCJ0aXRsZSI6IlZvaXIgbGUgYnVpbGQiLCJ1cmwiOiJodHRwczovL2dpdGh1Yi5jb20vIn0seyJ0eXBlIjoiQWN0aW9uLlN1Ym1pdCIsInRpdGxlIjoiUmVsYW5jZXIifV19fV19\"></Swift></URIObject>",
                ),
                (
                    30,
                    false,
                    "<p>Pense à garder l'auth derrière le trait <code>Authenticator</code>.</p>",
                ),
            ],
        ),
        (
            "19:claire_me@unq.gbl.spaces",
            "",
            ("8:orgid:claire", "Claire Dubois"),
            &[(1440, false, "<p>Réunion décalée à 15&nbsp;h demain.</p>")],
        ),
        (
            "19:general@thread.tacv2",
            "Équipe — Général",
            ("8:orgid:david", "David Moreau"),
            &[
                (2880, false, "<p>Bienvenue dans l'équipe !</p>"),
                (2000, false, "<p>N'oubliez pas le point hebdo vendredi.</p>"),
            ],
        ),
    ];

    let mut chats: Vec<Chat> = specs
        .into_iter()
        .map(|(id, topic, peer, msgs)| {
            let mut conversation = Conversation {
                id: id.into(),
                ..Default::default()
            };
            conversation.members.push(Member {
                id: peer.0.into(),
                extra: serde_json::Map::from_iter([("friendlyName".into(), json!(peer.1))]),
                ..Default::default()
            });
            if !topic.is_empty() {
                conversation
                    .thread_properties
                    .insert("topic".into(), json!(topic));
            }
            let messages = msgs
                .iter()
                .map(|&(ago, mine, html)| {
                    let (mri, name) = if mine { (ME, MY_NAME) } else { peer };
                    message(id, mri, name, html, &minutes_ago(ago))
                })
                .collect();
            Chat {
                conversation,
                peer,
                messages,
            }
        })
        .collect();
    for (id, name) in [
        ("19:design@thread.tacv2", "Design"),
        ("19:engineering@thread.tacv2", "Développement"),
        ("19:announcements@thread.tacv2", "Annonces"),
    ] {
        let messages = vec![message(
            id,
            "8:orgid:bob",
            "Bruno Lefèvre",
            &format!(
                "<p>Bienvenue dans le canal {name}. Partageons ici nos avancées et nos questions.</p>"
            ),
            &minutes_ago(20),
        )];
        chats.push(Chat {
            conversation: Conversation {
                id: id.into(),
                members: vec![
                    Member {
                        id: "8:orgid:bob".into(),
                        role: Some("Admin".into()),
                        ..Default::default()
                    },
                    Member {
                        id: "8:orgid:claire".into(),
                        ..Default::default()
                    },
                ],
                ..Default::default()
            },
            peer: ("8:orgid:bob", "Bruno Lefèvre"),
            messages,
        });
    }
    // A bot card laid out in columns, with a hidden column and an avatar that cannot load.
    if let Some(message) = chats
        .iter_mut()
        .find(|c| c.conversation.id == "19:engineering@thread.tacv2")
        .and_then(|c| c.messages.first_mut())
    {
        let text = |t: &str| json!({"type": "TextBlock", "text": t, "wrap": true});
        let stat = |t: &str| json!({"width": "auto", "items": [text(t)]});
        message.properties.insert("cards".into(), json!(json!([{
            "contentType": "application/vnd.microsoft.card.adaptive",
            "content": {"type": "AdaptiveCard", "body": [
                {"type": "ColumnSet", "columns": [
                    {"width": "auto", "items": [{"type": "Image", "url": "https://example.invalid/avatar.png", "size": "Small", "style": "Person"}]},
                    {"width": "stretch", "items": [text("[ci-bot](https://example.test/) wants to merge 4 commits from [fix/theme](https://example.test/) into [main](https://example.test/)")]}
                ]},
                {"type": "ColumnSet", "columns": [
                    stat("1 Reviewer ▼"),
                    {"width": "auto", "isVisible": false, "items": [text("1 Reviewer ▲")]},
                    stat("•"), stat("1 Comment"), stat("•"), stat("3 Files changed")
                ]}
            ]}
        }]).to_string()));
    }
    // A connector card as Workflows posts Uptime Kuma's alerts: a red accent and an activity image.
    if let Some(message) = chats
        .iter_mut()
        .find(|c| c.conversation.id == "19:engineering@thread.tacv2")
        .and_then(|c| c.messages.get_mut(1))
    {
        message.properties.insert("cards".into(), json!(json!([{
            "contentType": "application/vnd.microsoft.teams.card.o365connector",
            "content": {"@type": "MessageCard", "themeColor": "#ff0000", "summary": "down", "sections": [
                {"activityImage": "https://example.invalid/uptime-kuma.png", "activityTitle": "**Uptime Kuma**"},
                {"activityTitle": "🔴 Application [Volumes Backup] went down"},
                {"activityTitle": "**Description**", "text": "Request failed with status code 404",
                 "facts": [{"name": "Monitor", "value": "Volumes Backup"}, {"name": "URL", "value": "1"}]}
            ]}
        }]).to_string()));
    }
    // A link sent with its preview, as the web client sends one.
    if let Some(message) = chats
        .iter_mut()
        .find(|c| c.conversation.id == "19:alice_me@unq.gbl.spaces")
        .and_then(|c| c.messages.first_mut())
    {
        message.content = "<p>Salut ! Tu as vu la démo du nouveau client ? <a href=\"https://www.rust-lang.org/\">https://www.rust-lang.org/</a></p>".into();
        message.properties.insert("links".into(), json!(json!([{
            "@type": "http://schema.skype.com/HyperLink", "itemid": "0",
            "url": "https://www.rust-lang.org/", "previewenabled": true,
            "preview": {
                "previewurl": "https://eu-api.asm.skype.com/v1/objects/demo-image/views/imgo?n=rust",
                "previewmeta": {"width": 320, "height": 160},
                "title": "Rust Programming Language",
                "description": "A language empowering everyone to build reliable and efficient software."
            }
        }]).to_string()));
    }
    // A real file-shaped fixture exercises rendering without fabricating uploads.
    if let Some(chat) = chats
        .iter_mut()
        .find(|c| c.conversation.id == "19:projet-rust@thread.v2")
        && let Some(message) = chat.messages.first_mut()
    {
        message.properties.insert("files".into(),json!(json!([{"version":2,"title":"Documentation Rust","openUrl":null,"objectUrl":"https://doc.rust-lang.org/book/"},{"version":2,"id":"","title":"Feuille de route.txt","type":"txt","baseUrl":"{BASE}personal/me/","objectUrl":"{BASE}personal/me/Documents/Fichiers%20de%20conversation%20Microsoft%20Teams/Feuille%20de%20route.txt","state":"active"}]).to_string()));
        message.properties.insert(
            "emotions".into(),
            json!([{"key":"like","users":[{"mri":"8:orgid:claire"},{"mri":ME}]}]),
        );
        message.content.push_str("<p><a href='https://www.rust-lang.org/'>Site Rust</a> · https://doc.rust-lang.org/</p><img src='https://eu-api.asm.skype.com/v1/objects/demo-image/views/imgo' alt='Image de démonstration'>");
        chat.conversation.members.push(Member {
            id: "8:orgid:claire".into(),
            ..Default::default()
        });
        chat.conversation.is_one_on_one = Some(false);
        // `TEAMS_DEMO_HISTORY=600` gives this chat a long history, for measurements.
        let history: i64 = std::env::var("TEAMS_DEMO_HISTORY")
            .ok()
            .and_then(|n| n.parse().ok())
            .unwrap_or(0);
        let older = (0..history).rev().map(|i| {
            let mine = i % 3 == 0;
            let (mri, name) = if mine { (ME, MY_NAME) } else { chat.peer };
            let image = if i % 25 == 0 {
                format!("<img src='https://eu-api.asm.skype.com/v1/objects/demo-image/views/imgo?n={i}' alt='Capture {i}'>")
            } else {
                String::new()
            };
            let html = format!(
                "<p>Message d’historique n°{i} : <b>point</b> sur l’avancement, avec un peu de texte pour remplir la bulle.</p>{image}"
            );
            self::message(chat.conversation.id.as_str(), mri, name, &html, &minutes_ago(200 + i * 7))
        });
        chat.messages.splice(0..0, older.collect::<Vec<_>>());
    }
    Arc::new(Mutex::new(chats))
}

fn teams_data(base: &str) -> Value {
    let site = format!("{base}sites/demo");
    let general = crate::files_mock::GENERAL_FOLDER;
    let engineering = crate::files_mock::ENGINEERING_FOLDER;
    json!([
    {"id":"19:product-team@thread.tacv2","displayName":"Produit & Design","description":"Construisons une meilleure expérience.","teamSiteInformation":{"sharepointSiteUrl":site},"membershipSummary":{"totalMemberCount":12},"channels":[
        {"id":"19:general@thread.tacv2","displayName":"Général","isGeneral":true,"channelType":0,"defaultFileSettings":{"filesRelativePath":general}},
        {"id":"19:design@thread.tacv2","displayName":"Design","channelType":0}]},
    {"id":"19:engineering-team@thread.tacv2","displayName":"Engineering","teamSiteInformation":{"sharepointSiteUrl":site},"channels":[
        {"id":"19:engineering@thread.tacv2","displayName":"Développement","isGeneral":true,"channelType":0,"defaultFileSettings":{"filesRelativePath":engineering}},
        {"id":"19:announcements@thread.tacv2","displayName":"Annonces","channelType":1}]}])
}
fn chat_data(db: &[Chat]) -> Vec<Value> {
    db.iter().filter(|c|!c.conversation.id.contains("tacv2")).map(|c|{
    json!({"id":c.conversation.id,"chatSubType":0,"title":c.conversation.thread_properties.get("topic").and_then(Value::as_str).unwrap_or(""),
        "isOneOnOne":c.conversation.is_one_on_one.unwrap_or(true), "isRead":c.conversation.is_read.unwrap_or(c.peer.1!="Claire Dubois"),
        "isSticky":c.conversation.id=="19:projet-rust@thread.v2", "members":c.conversation.members.iter().map(|m|json!({"mri":m.id,"friendlyName":m.extra.get("friendlyName")})).chain(std::iter::once(json!({"mri":ME,"friendlyName":MY_NAME}))).collect::<Vec<_>>(),"lastMessage":c.messages.last()})
}).collect()
}
async fn startup(State(db): State<Db>, headers: HeaderMap) -> Json<Value> {
    Json(
        json!({"teams":teams_data(&crate::files_mock::base(&headers)),"chats":chat_data(&db.lock().unwrap()),"metadata":{"isPartialData":false,"syncToken":"demo"}}),
    )
}
async fn csa_chats(State(db): State<Db>) -> Json<Value> {
    Json(json!({"chats":chat_data(&db.lock().unwrap()),"hasMoreData":false}))
}
async fn team_channels(Path(team): Path<String>, headers: HeaderMap) -> Json<Value> {
    Json(
        teams_data(&crate::files_mock::base(&headers))
            .as_array()
            .unwrap()
            .iter()
            .find(|t| t["id"] == team)
            .map(|t| t["channels"].clone())
            .unwrap_or(json!([])),
    )
}
async fn posts(
    State(db): State<Db>,
    Path((_team, channel)): Path<(String, String)>,
) -> Result<Json<Value>, StatusCode> {
    let db = db.lock().unwrap();
    let chat = db
        .iter()
        .find(|c| c.conversation.id == channel)
        .ok_or(StatusCode::NOT_FOUND)?;
    let posts: Vec<Value> = chat
        .messages
        .iter()
        .filter(|m| !m.extra.contains_key("replyChainId"))
        .rev()
        .map(|root| {
            let messages: Vec<Value> = std::iter::once(root)
                .chain(
                    chat.messages
                        .iter()
                        .filter(|m| m.extra.get("replyChainId") == Some(&json!(root.id))),
                )
                .map(csa_message)
                .collect();
            json!({"id":root.id,"containerId":channel,"messages":messages})
        })
        .collect();
    Ok(Json(json!({"replyChains":posts})))
}
async fn replies(
    State(db): State<Db>,
    Path((_team, channel, post)): Path<(String, String, String)>,
) -> Result<Json<Value>, StatusCode> {
    let db = db.lock().unwrap();
    let chat = db
        .iter()
        .find(|c| c.conversation.id == channel)
        .ok_or(StatusCode::NOT_FOUND)?;
    let messages: Vec<_> = chat
        .messages
        .iter()
        .filter(|m| m.extra.get("replyChainId") == Some(&json!(post)))
        .map(csa_message)
        .collect();
    Ok(Json(json!({"messages":messages})))
}
async fn people(Json(query): Json<String>) -> Json<Value> {
    let q = query.to_lowercase();
    let values: Vec<_> = [("alice","Alice Martin"),("bob","Bruno Lefèvre"),("claire","Claire Dubois"),("david","David Moreau")].into_iter().filter(|(_,name)|name.to_lowercase().contains(&q)).map(|(id,name)|json!({"mri":format!("8:orgid:{id}"),"displayName":name,"email":format!("{id}@example.test")})).collect();
    Json(json!({"value":values}))
}
async fn create_chat(State(db): State<Db>, Json(body): Json<Value>) -> (StatusCode, HeaderMap) {
    let id = format!(
        "19:demo-{}@thread.v2",
        chrono::Utc::now().timestamp_micros()
    );
    let members: Vec<Member> = serde_json::from_value(body["members"].clone()).unwrap_or_default();
    let mut conversation = Conversation {
        id: id.clone(),
        members,
        ..Default::default()
    };
    conversation.thread_properties = body["properties"].as_object().cloned().unwrap_or_default();
    for m in &mut conversation.members {
        let name = match m.id.as_str() {
            ME => MY_NAME,
            "8:orgid:alice" => "Alice Martin",
            "8:orgid:bob" => "Bruno Lefèvre",
            "8:orgid:claire" => "Claire Dubois",
            _ => "David Moreau",
        };
        m.extra.insert("friendlyName".into(), json!(name));
    }
    db.lock().unwrap().push(Chat {
        conversation,
        peer: ("8:orgid:alice", "Alice Martin"),
        messages: vec![],
    });
    let mut headers = HeaderMap::new();
    headers.insert(
        "location",
        HeaderValue::from_str(&format!("/v1/threads/{id}")).unwrap(),
    );
    (StatusCode::CREATED, headers)
}
async fn edit_message(
    State(db): State<Db>,
    Path((conv, id)): Path<(String, String)>,
    Json(body): Json<Value>,
) -> StatusCode {
    let mut db = db.lock().unwrap();
    if let Some(chat) = db.iter_mut().find(|c| c.conversation.id == conv)
        && let Some(m) = chat
            .messages
            .iter_mut()
            .find(|m| m.id == id && m.sender_mri() == ME)
    {
        m.content = body["content"].as_str().unwrap_or_default().into();
        m.properties.insert("edittime".into(), json!(now_str()));
        return StatusCode::NO_CONTENT;
    }
    StatusCode::NOT_FOUND
}
async fn delete_message(
    State(db): State<Db>,
    Path((conv, id)): Path<(String, String)>,
) -> StatusCode {
    let mut db = db.lock().unwrap();
    if let Some(chat) = db.iter_mut().find(|c| c.conversation.id == conv)
        && let Some(m) = chat
            .messages
            .iter_mut()
            .find(|m| m.id == id && m.sender_mri() == ME)
    {
        m.content.clear();
        m.properties.insert("deletetime".into(), json!(now_str()));
        return StatusCode::NO_CONTENT;
    }
    StatusCode::NOT_FOUND
}
pub(crate) async fn mark_read(State(db): State<Db>, Path(conv): Path<String>) -> StatusCode {
    if let Some(c) = db
        .lock()
        .unwrap()
        .iter_mut()
        .find(|c| c.conversation.id == conv)
    {
        c.conversation.is_read = Some(true);
    }
    StatusCode::NO_CONTENT
}

/// The demo peer has read everything up to their own latest message.
async fn horizons(State(db): State<Db>, Path(conv): Path<String>) -> Json<Value> {
    let db = db.lock().unwrap();
    let horizons: Vec<Value> = db
        .iter()
        .find(|c| c.conversation.id == conv)
        .and_then(|chat| {
            let last = chat
                .messages
                .iter()
                .rev()
                .find(|m| m.sender_mri() == chat.peer.0)?;
            let at = chrono::DateTime::parse_from_rfc3339(last.originalarrivaltime.as_deref()?)
                .ok()?
                .timestamp_millis();
            Some(vec![json!({
                "id": chat.peer.0,
                "consumptionhorizon": format!("{at};{at};{}", last.id),
            })])
        })
        .unwrap_or_default();
    Json(json!({ "id": conv, "consumptionhorizons": horizons }))
}

async fn thread(State(db): State<Db>, Path(conv): Path<String>) -> Result<Json<Value>, StatusCode> {
    db.lock()
        .unwrap()
        .iter()
        .find(|c| c.conversation.id == conv)
        .map(|c| Json(json!(c.conversation)))
        .ok_or(StatusCode::NOT_FOUND)
}
async fn members(
    State(db): State<Db>,
    Path(conv): Path<String>,
) -> Result<Json<Value>, StatusCode> {
    db.lock()
        .unwrap()
        .iter()
        .find(|c| {
            c.conversation.id == conv
                || conv.contains("-team@thread.tacv2")
                    && c.conversation.id == "19:design@thread.tacv2"
        })
        .map(|c| {
            let mut members = c.conversation.members.clone();
            members.push(Member {
                id: ME.into(),
                ..Default::default()
            });
            Json(json!({"members":members}))
        })
        .ok_or(StatusCode::NOT_FOUND)
}
async fn profiles(Json(ids): Json<Vec<String>>) -> Json<Value> {
    Json(json!({"value":ids.into_iter().map(|id|{
        let name=if id==ME {MY_NAME}else if id.contains("claire"){"Claire Dubois"}else if id.contains("alice"){"Alice Martin"}else{"Bruno Lefèvre"};
        json!({"mri":id,"displayName":name})
    }).collect::<Vec<_>>()}))
}
async fn presence(Json(asked): Json<Vec<Value>>) -> Json<Value> {
    Json(json!(
        asked
            .iter()
            .filter_map(|entry| {
                let mri = entry["mri"].as_str()?;
                if mri.eq_ignore_ascii_case(ME) {
                    return Some(crate::manage_mock::my_presence(mri));
                }
                let (availability, activity) = if mri.contains("alice") {
                    ("Available", "Available")
                } else if mri.contains("bob") {
                    ("Busy", "InAMeeting")
                } else {
                    ("Away", "Away")
                };
                Some(json!({"mri": mri, "source": "ups", "status": 20000,
            "presence": {"availability": availability, "activity": activity}}))
            })
            .collect::<Vec<_>>()
    ))
}
/// Messages of the demo chats containing the query's words, newest first.
async fn search(State(db): State<Db>, Json(body): Json<Value>) -> Json<Value> {
    let query = body
        .pointer("/entityRequests/0/query/displayQueryString")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_lowercase();
    let db = db.lock().unwrap();
    let mut results: Vec<Value> = db
        .iter()
        .flat_map(|chat| chat.messages.iter().map(move |m| (chat, m)))
        .filter(|(_, m)| !query.is_empty() && crate::html::to_text(&m.content).to_lowercase().contains(&query))
        .map(|(chat, m)| {
            let channel = chat.conversation.id.contains("tacv2");
            json!({"Source": {
                "InternetMessageId": m.id, "ClientThreadId": chat.conversation.id,
                "ClientConversationId": if channel { format!("{};messageid={}", chat.conversation.id, m.id) } else { chat.conversation.id.clone() },
                "Preview": crate::html::to_text(&m.content), "DateTimeSent": m.originalarrivaltime,
                "Sender": {"EmailAddress": {"Name": m.imdisplayname.clone().unwrap_or_default()}},
                "Extensions": {"SkypeSpaces_ConversationPost_Extension_FromSkypeInternalId": m.from.rsplit('/').next().unwrap_or_default(),
                    "SkypeSpaces_ConversationPost_Extension_ThreadType": if channel { "topic" } else { "chat" }}}})
        })
        .collect();
    results.reverse();
    let total = results.len();
    Json(json!({"EntitySets": [{"ResultSets": [{"Total": total, "MoreResultsAvailable": false,
        "SearchTerms": query.split_whitespace().collect::<Vec<_>>(), "Results": results}]}]}))
}
/// The URL preview service: a title from the address, and the demo picture as its image.
async fn url_info(
    axum::extract::Query(query): axum::extract::Query<std::collections::HashMap<String, String>>,
) -> Result<Json<Value>, StatusCode> {
    let url = query.get("url").ok_or(StatusCode::BAD_REQUEST)?;
    let host = url::Url::parse(url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_owned))
        .ok_or(StatusCode::BAD_REQUEST)?;
    let site = host.trim_start_matches("www.");
    Ok(Json(json!({
        "url": url,
        "title": format!("{site} — page de démonstration"),
        "description": format!("Aperçu servi par le faux service d’aperçus pour {url}."),
        "thumbnail": format!("https://eu-api.asm.skype.com/v1/objects/demo-image/views/imgo?n={site}"),
        "thumbnail_meta": {"width": 320, "height": 160},
    })))
}
async fn demo_image() -> (HeaderMap, Vec<u8>) {
    // Generate a deterministic image fixture through the same authenticated AMS route.
    let image = image::RgbImage::from_fn(320, 160, |x, y| {
        image::Rgb([90 + (x / 4) as u8, 90 + (y / 2) as u8, 210])
    });
    let mut data = std::io::Cursor::new(Vec::new());
    image.write_to(&mut data, image::ImageFormat::Png).unwrap();
    let mut headers = HeaderMap::new();
    headers.insert("content-type", HeaderValue::from_static("image/png"));
    (headers, data.into_inner())
}

async fn add_reaction(
    State(db): State<Db>,
    Path((conv, id)): Path<(String, String)>,
    Json(body): Json<Value>,
) -> StatusCode {
    reaction_change(db, conv, id, body, false)
}
async fn remove_reaction(
    State(db): State<Db>,
    Path((conv, id)): Path<(String, String)>,
    Json(body): Json<Value>,
) -> StatusCode {
    reaction_change(db, conv, id, body, true)
}
fn reaction_change(db: Db, conv: String, id: String, body: Value, remove: bool) -> StatusCode {
    if conv == teams_calling::history::CALL_LOGS && body.get("call-log").is_some() {
        return crate::calls_mock::delete_log(&id);
    }
    let mut db = db.lock().unwrap();
    let Some(message) = db
        .iter_mut()
        .find(|c| c.conversation.id == conv)
        .and_then(|c| c.messages.iter_mut().find(|m| m.id == id))
    else {
        return StatusCode::NOT_FOUND;
    };
    if message.is_deleted() {
        return StatusCode::NOT_FOUND;
    }
    let Some(key) = body["emotions"]["key"].as_str() else {
        return StatusCode::BAD_REQUEST;
    };
    message.apply_reaction(ME, key, remove);
    StatusCode::NO_CONTENT
}

/// CSA message schema is distinct from the Chat Service response used for DMs.
fn csa_message(message: &Message) -> Value {
    let mut value = json!(message);
    let object = value.as_object_mut().unwrap();
    for (old, new) in [
        ("messagetype", "messageType"),
        ("originalarrivaltime", "originalArrivalTime"),
        ("composetime", "composeTime"),
        ("clientmessageid", "clientMessageId"),
    ] {
        if let Some(value) = object.remove(old) {
            object.insert(new.into(), value);
        }
    }
    object.insert(
        "version".into(),
        json!(
            message
                .version
                .as_deref()
                .and_then(|v| v.parse::<u64>().ok())
                .unwrap_or(0)
        ),
    );
    value
}

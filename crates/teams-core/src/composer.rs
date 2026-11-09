//! Composer tokens retain their Teams identity while the plain-text draft is edited.
use super::*;

#[derive(Debug, Clone)]
pub(super) enum TokenKind {
    Mention(String),
    Emoji(String),
}
#[derive(Debug, Clone)]
pub(super) struct Token {
    start: usize,
    end: usize,
    text: String,
    kind: TokenKind,
}
impl Token {
    pub(super) fn new(start: usize, text: String, kind: TokenKind) -> Self {
        Self {
            start,
            end: start + text.len(),
            text,
            kind,
        }
    }
}
#[derive(Default)]
pub(super) struct ComposerState {
    pub tokens: HashMap<String, Vec<Token>>,
    pub formats: HashMap<String, Vec<super::formatting::ComposerFormat>>,
    /// What the message being edited sends back as posted.
    pub edited: Option<super::rich_edit::Posted>,
}
#[derive(Debug, Clone, PartialEq, Eq, Hash, uniffi::Record)]
pub struct MentionRange {
    pub start: u32,
    pub length: u32,
}
/// AppKit ranges count UTF-16 units. Never split a surrogate pair or accept stale ranges.
pub(super) fn byte_offset(text: &str, offset: u32) -> Option<usize> {
    let mut units = 0;
    for (byte, ch) in text.char_indices() {
        if units == offset {
            return Some(byte);
        }
        units += ch.len_utf16() as u32;
        if units > offset {
            return None;
        }
    }
    (units == offset).then_some(text.len())
}
impl App {
    pub(super) fn mention_ranges(&self) -> Vec<MentionRange> {
        self.valid_tokens()
            .into_iter()
            .filter(|t| matches!(t.kind, TokenKind::Mention(_)))
            .map(|t| MentionRange {
                start: self.conv.draft()[..t.start].encode_utf16().count() as u32,
                length: t.text.encode_utf16().count() as u32,
            })
            .collect()
    }
    pub(crate) fn mention_members_cached(&self) -> bool {
        self.selected
            .as_ref()
            .is_some_and(|s| self.channel_members.contains_key(&s.key()))
    }
    pub(super) fn mention_people(&self) -> Vec<view::PersonView> {
        let Some(selection) = &*self.selected else {
            return vec![];
        };
        let members = self
            .channel_members
            .get(&selection.key())
            .map(Vec::as_slice)
            .or_else(|| {
                self.chats
                    .iter()
                    .find(|c| c.id == selection.id())
                    .map(|c| c.members.as_slice())
            });
        let mut seen = HashSet::new();
        let mut people: Vec<_> = members
            .unwrap_or_default()
            .iter()
            .filter(|m| !m.id.eq_ignore_ascii_case(&self.me) && seen.insert(m.id.to_lowercase()))
            .filter_map(|m| {
                let profile = self.profiles.get(&m.id);
                let name = profile
                    .and_then(Person::known_name)
                    .or_else(|| m.display_name())?;
                Some(view::PersonView {
                    mri: m.id.clone(),
                    name: name.into(),
                    email: profile.map(|p| p.email.clone()).unwrap_or_default(),
                })
            })
            .collect();
        people.sort_by_cached_key(|p| p.name.to_lowercase());
        people
    }
    pub(super) fn replace_draft(
        &mut self,
        start: u32,
        length: u32,
        text: String,
        mention: Option<String>,
        emoji: Option<String>,
    ) {
        if self.read_only() {
            return;
        }
        let Some(end_units) = start.checked_add(length) else {
            return;
        };
        let (Some(start_byte), Some(end_byte)) = (
            byte_offset(self.conv.draft(), start),
            byte_offset(self.conv.draft(), end_units),
        ) else {
            return;
        };
        let kind = if let Some(mri) = mention {
            // Only selected participants can become notification-bearing mentions.
            if !self
                .mention_people()
                .iter()
                .any(|p| p.mri == mri && text == format!("@{} ", p.name))
            {
                return;
            }
            Some(TokenKind::Mention(mri))
        } else if let Some(id) = emoji {
            if !crate::emoji::is_emoticon(&id, &text) {
                return;
            }
            Some(TokenKind::Emoji(id))
        } else {
            None
        };
        let mut value = self.conv.draft().to_owned();
        value.replace_range(start_byte..end_byte, &text);
        // Use the exact selected range, even when the surrounding text repeats.
        self.shift_tokens(start_byte, end_byte, text.len());
        self.shift_formats(start, end_units, text.encode_utf16().count() as u32);
        if let Some(kind) = kind {
            let token_text = if matches!(kind, TokenKind::Mention(_)) {
                text.trim_end().to_owned()
            } else {
                text.clone()
            };
            let key = self.draft_key();
            self.composer.tokens.entry(key).or_default().push(Token {
                start: start_byte,
                end: start_byte + token_text.len(),
                text: token_text,
                kind,
            });
        }
        self.conv.set_draft(value);
        self.conv
            .set_cursor(start + text.encode_utf16().count() as u32);
    }
    fn shift_tokens(&mut self, start: usize, end: usize, replacement: usize) {
        let key = self.draft_key();
        if let Some(tokens) = self.composer.tokens.get_mut(&key) {
            tokens.retain_mut(|token| {
                if token.end <= start {
                    true
                } else if token.start >= end {
                    token.start = token.start - (end - start) + replacement;
                    token.end = token.end - (end - start) + replacement;
                    true
                } else {
                    false
                }
            });
        }
    }
    pub(super) fn track_draft_edit(&mut self, value: &str) {
        // Conservative common-prefix/suffix diff: editing inside a mention removes its identity.
        let prefix = self
            .conv
            .draft()
            .chars()
            .zip(value.chars())
            .take_while(|(a, b)| a == b)
            .map(|(c, _)| c.len_utf8())
            .sum::<usize>();
        let suffix = self.conv.draft()[prefix..]
            .chars()
            .rev()
            .zip(value[prefix..].chars().rev())
            .take_while(|(a, b)| a == b)
            .map(|(c, _)| c.len_utf8())
            .sum::<usize>();
        self.shift_tokens(
            prefix,
            self.conv.draft().len() - suffix,
            value.len() - prefix - suffix,
        );
        self.shift_formats(
            self.conv.draft()[..prefix].encode_utf16().count() as u32,
            self.conv.draft()[..self.conv.draft().len() - suffix]
                .encode_utf16()
                .count() as u32,
            value[prefix..value.len() - suffix].encode_utf16().count() as u32,
        );
    }
    fn valid_tokens(&self) -> Vec<&Token> {
        let mut tokens: Vec<_> = self
            .composer
            .tokens
            .get(&self.draft_key())
            .into_iter()
            .flatten()
            .filter(|t| self.conv.draft().get(t.start..t.end) == Some(t.text.as_str()))
            .collect();
        tokens.sort_by_key(|t| t.start);
        tokens
    }
    pub(super) fn composer_mentions(&self) -> String {
        let mentions: Vec<_> = self
            .valid_tokens()
            .into_iter()
            .filter_map(|t| {
                if let TokenKind::Mention(mri) = &t.kind {
                    Some(serde_json::json!({
                        "@type":"http://schema.skype.com/Mention", "itemid": "", "mri":mri,
                        "mentionType":"person", "displayName":t.text.trim_start_matches('@')
                    }))
                } else {
                    None
                }
            })
            .enumerate()
            .map(|(i, mut m)| {
                m["itemid"] = i.to_string().into();
                m
            })
            .collect();
        serde_json::to_string(&mentions).expect("mention JSON")
    }
    pub(super) fn composer_html(&self) -> String {
        self.composer_rich_html()
    }
    pub(super) fn composer_inline_html(&self, start: usize, end: usize) -> String {
        let mut html = String::new();
        let mut cursor = start;
        let mut mention_id = 0;
        for token in self.valid_tokens() {
            let id = mention_id;
            if matches!(token.kind, TokenKind::Mention(_)) {
                mention_id += 1;
            }
            if token.start < cursor || token.end > end {
                continue;
            }
            html.push_str(&self.styled_text(cursor, token.start));
            let value = match &token.kind {
                TokenKind::Mention(_) => format!(
                    "<span itemtype=\"http://schema.skype.com/Mention\" itemscope=\"\" itemid=\"{id}\">{}</span>",
                    escape(token.text.trim_start_matches('@'))
                ),
                TokenKind::Emoji(id) => {
                    format!("<ss type=\"{}\">{}</ss>", escape(id), escape(&token.text))
                }
            };
            html.push_str(&super::formatting::styled(
                value,
                self.style_at(token.start),
            ));
            cursor = token.end;
        }
        html.push_str(&self.styled_text(cursor, end));
        html
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn app() -> App {
        let mut app = App::empty();
        *app.selected = Some(Selection::Chat("chat".into()));
        app.chats.push(serde_json::from_value(serde_json::json!({"id":"chat", "members":[{"id":"8:orgid:alice","displayName":"Alice & Bob"}]})).unwrap());
        app
    }
    #[test]
    fn mention_survives_prefix_edit_but_not_edit_inside_and_stays_with_its_draft() {
        let mut app = app();
        app.conv.draft_typed("😀 @al fin".into());
        app.replace_draft(
            3,
            3,
            "@Alice & Bob ".into(),
            Some("8:orgid:alice".into()),
            None,
        );
        assert_eq!(app.conv.draft(), "😀 @Alice & Bob  fin");
        assert_eq!(app.conv.cursor(), 16);
        assert!(
            app.composer_html()
                .contains("itemid=\"0\">Alice &amp; Bob</span>")
        );
        let mentions: serde_json::Value = serde_json::from_str(&app.composer_mentions()).unwrap();
        assert_eq!(mentions[0]["mri"], "8:orgid:alice");
        app.track_draft_edit("Bonjour 😀 @Alice & Bob  fin");
        app.conv.draft_typed("Bonjour 😀 @Alice & Bob  fin".into());
        assert!(app.composer_html().contains("<span"));
        let _ = app.update(Msg::Select(Selection::Chat("other".into())));
        assert_eq!(app.composer_mentions(), "[]");
        let _ = app.update(Msg::Select(Selection::Chat("chat".into())));
        assert!(app.composer_html().contains("<span"));
        app.track_draft_edit("Bonjour 😀 @Alicia & Bob  fin");
        app.conv.draft_typed("Bonjour 😀 @Alicia & Bob  fin".into());
        assert_eq!(app.composer_mentions(), "[]");
    }
    #[test]
    fn rejects_invalid_utf16_and_people_outside_roster_and_preserves_emoji_id() {
        let mut app = app();
        app.conv.draft_typed("😀test".into());
        app.replace_draft(1, 0, "x".into(), None, None);
        assert_eq!(app.conv.draft(), "😀test");
        app.replace_draft(2, 0, "@Intrus ".into(), Some("outsider".into()), None);
        assert_eq!(app.conv.draft(), "😀test");
        let emoji = crate::emoji::categories()[0].items[0].clone();
        app.replace_draft(2, 4, emoji.text.clone(), None, Some(emoji.id.clone()));
        assert!(
            app.composer_html()
                .contains(&format!("<ss type=\"{}\">{}</ss>", emoji.id, emoji.text))
        );
        assert_eq!(app.composer_mentions(), "[]");
    }
    #[tokio::test]
    async fn send_preserves_teams_mentions_and_emoji_in_chat_posts_and_replies() {
        use axum::{Json, Router, routing::post};
        use serde_json::{Value, json};
        let received = Arc::new(std::sync::Mutex::new(Vec::<Value>::new()));
        let captured = received.clone();
        let router = Router::new().route(
            "/v1/users/ME/conversations/{conv}/messages",
            post(move |Json(body): Json<Value>| {
                let captured = captured.clone();
                async move {
                    captured.lock().unwrap().push(body);
                    Json(json!({"id":"sent"}))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let auth = StaticAuthenticator::new().with_service(Service::ChatService, base);
        for (channel, reply) in [(false, None), (true, None), (true, Some("post"))] {
            let mut app = app();
            *app.client = Some(ChatClient::new(Arc::new(AppAuthenticator::Demo(
                auth.clone(),
            ))));
            if channel {
                *app.selected = Some(Selection::Channel {
                    team: "team".into(),
                    channel: "channel".into(),
                });
                app.channel_members.insert(
                    app.selected.as_ref().unwrap().key(),
                    app.chats[0].members.clone(),
                );
            }
            *app.conv.test_reply() = reply.map(str::to_owned);
            app.replace_draft(
                0,
                0,
                "@Alice & Bob ".into(),
                Some("8:orgid:alice".into()),
                None,
            );
            let emoji = &crate::emoji::categories()[0].items[0];
            app.replace_draft(
                app.conv.cursor(),
                0,
                emoji.text.clone(),
                None,
                Some(emoji.id.clone()),
            );
            let task = app.handle(Msg::Send);
            for job in task.0 {
                if let crate::task::Job::Future(future) = job {
                    assert!(matches!(
                        future.await,
                        Msg::Sent {
                            generation: _,
                            selection: _,
                            reply: _,
                            content: _,
                            ids: _,
                            result: Ok(())
                        }
                    ));
                }
            }
            let body = received.lock().unwrap().last().unwrap().clone();
            let mentions: Value =
                serde_json::from_str(body["properties"]["mentions"].as_str().unwrap()).unwrap();
            assert_eq!(mentions[0]["itemid"], "0");
            assert_eq!(mentions[0]["mri"], "8:orgid:alice");
            assert!(
                body["content"]
                    .as_str()
                    .unwrap()
                    .contains("itemid=\"0\">Alice &amp; Bob</span>")
            );
            assert!(
                body["content"]
                    .as_str()
                    .unwrap()
                    .contains(&format!("<ss type=\"{}\">", emoji.id))
            );
            assert_eq!(body.get("replyChainId").and_then(Value::as_str), reply);
        }
        server.abort();
    }
    #[test]
    fn send_completion_keeps_mentions_added_to_a_newer_draft() {
        let mut app = app();
        app.replace_draft(
            0,
            0,
            "@Alice & Bob ".into(),
            Some("8:orgid:alice".into()),
            None,
        );
        let _ = app.handle(Msg::Sent {
            generation: 0,
            selection: Selection::Chat("chat".into()),
            reply: None,
            content: "older sent content".into(),
            ids: vec![],
            result: Ok(()),
        });
        assert!(app.composer_html().contains("<span"));
        let _ = app.handle(Msg::Sent {
            generation: 0,
            selection: Selection::Chat("chat".into()),
            reply: None,
            content: app.conv.draft().to_owned(),
            ids: vec![],
            result: Err("offline".into()),
        });
        assert!(app.composer_html().contains("<span"));
    }
}

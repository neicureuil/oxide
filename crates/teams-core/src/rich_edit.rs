//! A sent message loaded back into the composer's rich draft, so it is edited with the editor it was written in.
use super::composer::{Token, TokenKind};
use super::formatting::{ComposerBlock, ComposerFormat, IMAGE_PLACEHOLDER};
use super::*;
use crate::html::{self, Style};
use scraper::{ElementRef, Html, Node};
use serde_json::Value;

/// The draft form of a message. Its images stand in the text as pasted ones do, sent back as they were.
#[derive(Debug, Default)]
pub(super) struct LoadedDraft {
    pub text: String,
    pub formats: Vec<ComposerFormat>,
    pub tokens: Vec<Token>,
    pub posted: Posted,
}
/// What an edited message sends back as it was posted.
/// What the draft cannot hold stays HTML: a quoted reply goes back before the edited text, tables and cards after it.
#[derive(Debug, Default)]
pub(super) struct Posted {
    /// Each image's `<img>`, by the id its format holds.
    pub images: HashMap<String, String>,
    pub before: String,
    pub after: String,
}
/// The draft text of a horizontal rule, as the formatting toolbar inserts it.
const RULE: &str = "────────";

#[derive(Debug, Clone, Default, PartialEq)]
struct Line {
    block: ComposerBlock,
    indent: u32,
    alignment: String,
}
#[derive(Clone, Default)]
struct Context {
    style: Style,
    line: Line,
    list: Option<ComposerBlock>,
}
struct Builder<'a> {
    mentions: &'a [Value],
    text: String,
    /// One format per run of identical style, in UTF-16 units; a line's style covers its ending newline.
    formats: Vec<ComposerFormat>,
    /// The text's length in UTF-16 units.
    units: u32,
    tokens: Vec<Token>,
    posted: Posted,
    /// The current line's style, set by its first content.
    line: Option<Line>,
    /// Collapsed whitespace, written only if more text follows on the line.
    space: Option<ComposerFormat>,
}

pub(super) fn load(message: &Message) -> LoadedDraft {
    let mentions = notifications::mentions(message);
    let mut builder = Builder {
        mentions: &mentions,
        text: String::new(),
        formats: Vec::new(),
        units: 0,
        tokens: Vec::new(),
        posted: Posted::default(),
        line: None,
        space: None,
    };
    if message.messagetype == "Text" {
        for (i, line) in message.content.lines().enumerate() {
            if i > 0 {
                builder.newline(&Line::default());
            }
            builder.push(line, &ComposerFormat::default(), &Line::default());
        }
    } else {
        let tree = Html::parse_fragment(&message.content);
        builder.children(tree.root_element(), &Context::default());
    }
    builder.finish()
}

impl Builder<'_> {
    fn children(&mut self, element: ElementRef<'_>, context: &Context) {
        for child in element.children() {
            match child.value() {
                Node::Text(text) => self.append(text, &inline(&context.style), &context.line),
                Node::Element(_) => {
                    if let Some(el) = ElementRef::wrap(child) {
                        self.walk(el, context);
                    }
                }
                _ => {}
            }
        }
    }
    fn walk(&mut self, element: ElementRef<'_>, parent: &Context) {
        let tag = element.value().name();
        if matches!(
            tag,
            "script" | "style" | "iframe" | "object" | "noscript" | "head" | "legacyquote"
        ) {
            return;
        }
        let mut context = parent.clone();
        html::css(element, &mut context.style);
        match (tag, html::itemtype(element)) {
            (_, "Emoji" | "CustomEmoji") | ("emoji" | "ss", _) => {
                return self.emoji(element, &context);
            }
            (_, "Mention") | ("at", _) => return self.mention(element, &context),
            ("blockquote" | "quote", _) if is_reply(element) => {
                self.posted.before.push_str(&element.html());
                return;
            }
            ("img", _) if html::image_src(element).is_some() => {
                return self.image(element, &context);
            }
            ("img" | "table" | "uriobject", _) => {
                self.posted.after.push_str(&element.html());
                return;
            }
            ("pre" | "codeblock", _) => return self.code(element, &context),
            ("hr", _) => {
                let rule = Line {
                    block: ComposerBlock::Rule,
                    ..Line::default()
                };
                self.break_line(&parent.line);
                self.push(RULE, &ComposerFormat::default(), &rule);
                self.newline(&rule);
                return;
            }
            ("br", _) => return self.newline(&context.line),
            ("ul" | "ol", _) => {
                if context.list.is_some() {
                    context.line.indent += 1;
                }
                context.line.indent += indent(element);
                context.list = Some(if tag == "ol" {
                    ComposerBlock::Numbered
                } else {
                    ComposerBlock::Bullet
                });
            }
            ("a", _) => context.style.link = element.value().attr("href").and_then(html::safe_link),
            ("b" | "strong", _) => context.style.bold = true,
            ("em" | "i", _) => context.style.italic = true,
            ("u", _) => context.style.underline = true,
            ("s" | "del" | "strike", _) => context.style.strike = true,
            ("code", _) => context.style.code = true,
            _ => {}
        }
        let block = match tag {
            "h1" => Some(ComposerBlock::Heading1),
            "h2" => Some(ComposerBlock::Heading2),
            "h3" | "h4" | "h5" | "h6" => Some(ComposerBlock::Heading3),
            "li" => Some(context.list.unwrap_or(ComposerBlock::Bullet)),
            "blockquote" | "quote" => Some(ComposerBlock::Quote),
            "p" | "div" => Some(context.line.block),
            _ => None,
        };
        let Some(block) = block else {
            return self.children(element, &context);
        };
        context.line.block = block;
        context.line.indent += indent(element);
        if let Some(alignment) = alignment(element) {
            context.line.alignment = alignment.into();
        }
        self.break_line(&parent.line);
        self.children(element, &context);
        self.break_line(&context.line);
    }
    fn emoji(&mut self, element: ElementRef<'_>, context: &Context) {
        let attrs = element.value();
        let text = html::emoji_text(element);
        let id = attrs
            .attr("itemid")
            .or(attrs.attr("id"))
            .or(attrs.attr("type"))
            .unwrap_or_default();
        let style = inline(&context.style);
        if crate::emoji::is_emoticon(id, &text) {
            self.token(text, TokenKind::Emoji(id.into()), &style, &context.line);
        } else {
            self.append(&text, &style, &context.line);
        }
    }
    /// Keyed by its picture's address, which is also where the app keeps the picture it shows.
    fn image(&mut self, element: ElementRef<'_>, context: &Context) {
        let Some(src) = html::image_src(element) else {
            return;
        };
        self.posted.images.insert(src.to_owned(), element.html());
        let style = ComposerFormat {
            image: Some(src.to_owned()),
            ..inline(&context.style)
        };
        self.push(&IMAGE_PLACEHOLDER.to_string(), &style, &context.line);
    }
    fn mention(&mut self, element: ElementRef<'_>, context: &Context) {
        let name = element.text().collect::<String>();
        let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
        let attrs = element.value();
        let id = attrs
            .attr("itemid")
            .or(attrs.attr("id"))
            .unwrap_or_default();
        let style = inline(&context.style);
        match person(self.mentions, id) {
            Some(mri) if !name.is_empty() => self.token(
                format!("@{name}"),
                TokenKind::Mention(mri),
                &style,
                &context.line,
            ),
            _ => self.append(&name, &style, &context.line),
        }
    }
    fn code(&mut self, element: ElementRef<'_>, context: &Context) {
        let mut text = String::new();
        html::verbatim(element, &mut text);
        let line = Line {
            block: ComposerBlock::CodeBlock,
            ..context.line.clone()
        };
        self.break_line(&context.line);
        for (i, part) in text.trim_matches('\n').trim_end().split('\n').enumerate() {
            if i > 0 {
                self.newline(&line);
            }
            self.push(part, &ComposerFormat::default(), &line);
        }
        self.break_line(&line);
    }
    /// Text as a browser lays it out: whitespace runs collapse, and none starts or ends a line.
    fn append(&mut self, value: &str, style: &ComposerFormat, line: &Line) {
        let mut run = String::new();
        for ch in value.chars() {
            if ch.is_whitespace() {
                if !run.is_empty() {
                    self.push(&run, style, line);
                    run.clear();
                }
                if !self.at_line_start() {
                    self.space = Some(style.clone());
                }
            } else {
                run.push(ch);
            }
        }
        self.push(&run, style, line);
    }
    fn token(&mut self, text: String, kind: TokenKind, style: &ComposerFormat, line: &Line) {
        self.push("", style, line);
        let start = self.text.len();
        self.push(&text, style, line);
        self.tokens.push(Token::new(start, text, kind));
    }
    fn push(&mut self, value: &str, style: &ComposerFormat, line: &Line) {
        if let Some(space) = self.space.take() {
            self.push_run(" ", &space, line);
        }
        self.push_run(value, style, line);
    }
    fn push_run(&mut self, value: &str, style: &ComposerFormat, line: &Line) {
        if value.is_empty() {
            return;
        }
        let line = self.line.get_or_insert_with(|| line.clone()).clone();
        self.text.push_str(value);
        self.add(value, style, &line);
    }
    fn at_line_start(&self) -> bool {
        self.text.is_empty() || self.text.ends_with('\n')
    }
    fn break_line(&mut self, line: &Line) {
        if !self.at_line_start() {
            self.newline(line);
        }
    }
    fn newline(&mut self, line: &Line) {
        self.space = None;
        let finished = self.line.take().unwrap_or_else(|| line.clone());
        self.text.push('\n');
        self.add("\n", &ComposerFormat::default(), &finished);
    }
    fn add(&mut self, piece: &str, inline: &ComposerFormat, line: &Line) {
        let length = piece.encode_utf16().count() as u32;
        let format = ComposerFormat {
            start: self.units,
            length,
            block: line.block,
            indent: line.indent,
            alignment: line.alignment.clone(),
            ..inline.clone()
        };
        self.units += length;
        let plain = ComposerFormat {
            start: format.start,
            length,
            ..Default::default()
        };
        if format == plain {
            return;
        }
        let continues = |last: &ComposerFormat| {
            let moved = ComposerFormat {
                start: format.start,
                length,
                ..last.clone()
            };
            last.start + last.length == format.start && moved == format && format.image.is_none()
        };
        if let Some(last) = self.formats.last_mut()
            && continues(last)
        {
            last.length += length;
            return;
        }
        self.formats.push(format);
    }
    fn finish(mut self) -> LoadedDraft {
        while self.text.ends_with('\n') {
            self.text.pop();
            self.units -= 1;
            if let Some(last) = self.formats.last_mut()
                && last.start + last.length > self.units
            {
                last.length -= 1;
                if last.length == 0 {
                    self.formats.pop();
                }
            }
        }
        LoadedDraft {
            formats: self.formats,
            text: self.text,
            tokens: self.tokens,
            posted: self.posted,
        }
    }
}
fn inline(style: &Style) -> ComposerFormat {
    ComposerFormat {
        bold: style.bold,
        italic: style.italic,
        underline: style.underline,
        strike: style.strike,
        code: style.code,
        color: style.color.as_deref().map(long_hex),
        highlight: style.highlight.as_deref().map(long_hex),
        size: style.size,
        link: style.link.clone(),
        ..Default::default()
    }
}
/// `#rgb` as `#rrggbb`, the only form the draft keeps.
fn long_hex(color: &str) -> String {
    if color.len() != 4 {
        return color.to_owned();
    }
    color[1..]
        .chars()
        .fold(String::from("#"), |mut hex, digit| {
            hex.push(digit);
            hex.push(digit);
            hex
        })
}
/// A quote of another message, which only points at it: the editor leaves it as it is.
fn is_reply(element: ElementRef<'_>) -> bool {
    let attrs = element.value();
    attrs.name() == "quote"
        || html::itemtype(element) == "Reply"
        || attrs.attr("authorname").is_some()
        || attrs.attr("itemid").is_some()
}
/// The person a mention's `itemid` names; tag, team and channel mentions stay text.
fn person(mentions: &[Value], id: &str) -> Option<String> {
    let mention = mentions.iter().find(|m| match &m["itemid"] {
        Value::String(s) => s == id,
        Value::Number(n) => n.to_string() == id,
        _ => false,
    })?;
    if !matches!(mention["mentionType"].as_str(), None | Some("person")) {
        return None;
    }
    mention["mri"]
        .as_str()
        .filter(|mri| !mri.is_empty())
        .map(str::to_owned)
}
fn declaration<'a>(element: ElementRef<'a>, name: &str) -> Option<&'a str> {
    element.value().attr("style")?.split(';').find_map(|d| {
        let (key, value) = d.split_once(':')?;
        key.trim().eq_ignore_ascii_case(name).then(|| value.trim())
    })
}
/// The composer writes each indent level as 24px of left margin.
fn indent(element: ElementRef<'_>) -> u32 {
    declaration(element, "margin-left")
        .and_then(|v| v.strip_suffix("px"))
        .and_then(|v| v.trim().parse::<f64>().ok())
        .map_or(0, |px| (px / 24.0).round().max(0.0) as u32)
}
fn alignment(element: ElementRef<'_>) -> Option<&'static str> {
    match declaration(element, "text-align")? {
        "center" => Some("center"),
        "right" => Some("right"),
        _ => None,
    }
}

impl App {
    pub(super) fn begin_edit(&mut self, id: String) -> Task<Msg> {
        let Some(message) = self
            .conv
            .find_message(&id)
            .filter(|m| m.sender_mri() == *self.me)
        else {
            return Task::none();
        };
        let draft = load(message);
        let ended = self.end_edit();
        self.conv.begin_edit(self.selected.as_ref(), id);
        self.load_draft(draft);
        ended
    }
    /// Gives the draft put aside back, forgetting the edit and the images pasted into it.
    pub(super) fn end_edit(&mut self) -> Task<Msg> {
        if self.conv.editing().is_none() {
            return Task::none();
        }
        let key = self.draft_key();
        self.composer.tokens.remove(&key);
        self.composer.formats.remove(&key);
        self.composer.edited = None;
        let discarded = self.discard_attachments();
        self.conv.end_edit(self.selected.as_ref());
        discarded
    }
    /// Sends the edited draft as the message's new content; the edit ends once it is accepted.
    pub(super) fn save_edit(&mut self) -> Task<Msg> {
        let (Some(id), Some(selection), Some(client)) = (
            self.conv.editing().cloned(),
            self.selected.clone(),
            self.client.clone(),
        ) else {
            return Task::none();
        };
        self.drop_unplaced_images();
        if self.conv.draft().trim().is_empty() {
            return Task::none();
        }
        if let Err(notice) = self.ready_attachments() {
            *self.notice = Some(notice);
            return Task::none();
        }
        let Some(html) = self.edited_html() else {
            return Task::none();
        };
        let mut message =
            OutgoingMessage::html(client_message_id(), html.clone(), &*self.display_name);
        message.properties.mentions = self.composer_mentions();
        let generation = self.generation;
        Task::perform(
            async move {
                let result = client
                    .edit_message(selection.id(), &id, &message)
                    .await
                    .map(drop)
                    .map_err(|e| e.to_string());
                (selection, id, result)
            },
            move |(selection, id, result)| Msg::EditSaved {
                generation,
                selection,
                id,
                html,
                result,
            },
        )
    }
    /// The edited message's new HTML: its draft between what the draft could not hold.
    fn edited_html(&self) -> Option<String> {
        let posted = self.composer.edited.as_ref()?;
        Some(format!(
            "{}{}{}",
            posted.before,
            self.composer_html(),
            posted.after
        ))
    }
    /// Ends the edit once Teams accepted it, unless the draft changed since it was saved, then shows the message as saved.
    pub(super) fn edit_saved(
        &mut self,
        generation: u64,
        selection: Selection,
        id: String,
        html: String,
        result: Result<(), String>,
    ) -> Task<Msg> {
        let unchanged = generation == self.generation
            && self.conv.editing() == Some(&id)
            && self.edited_html() == Some(html);
        let ended = if result.is_ok() && unchanged {
            self.end_edit()
        } else {
            Task::none()
        };
        let saved = self.handle(ConversationMsg::Mutated {
            generation,
            selection,
            result,
        });
        Task::batch([ended, saved])
    }
    /// An image of the edited message, as it was posted.
    pub(super) fn posted_image(&self, id: &str) -> Option<&str> {
        self.composer
            .edited
            .as_ref()?
            .images
            .get(id)
            .map(String::as_str)
    }
    /// Replaces the current draft with the edited message's `draft`, keeping what it sends back as posted.
    pub(super) fn load_draft(&mut self, draft: LoadedDraft) {
        let key = self.draft_key();
        self.rich_draft_changed(draft.text.clone(), draft.formats);
        self.conv.set_draft(draft.text);
        self.composer.tokens.insert(key, draft.tokens);
        self.composer.edited = Some(draft.posted);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::harness::Harness;
    use files::FilesMsg;
    fn app() -> App {
        let mut app = App::empty();
        *app.selected = Some(Selection::Chat("chat".into()));
        app.chats.push(serde_json::from_value(serde_json::json!({"id":"chat", "members":[{"id":"8:orgid:alice","displayName":"Alice & Bob"}]})).unwrap());
        app
    }
    fn message(content: &str, mentions: &str) -> Message {
        serde_json::from_value(serde_json::json!({
            "id": "1",
            "messagetype": "RichText/Html",
            "content": content,
            "properties": {"mentions": mentions},
        }))
        .unwrap()
    }
    fn format(start: u32, length: u32) -> ComposerFormat {
        ComposerFormat {
            start,
            length,
            ..Default::default()
        }
    }
    /// Sends `app`'s draft and loads it back into a fresh draft.
    fn reloaded(app: &App) -> (App, LoadedDraft) {
        let sent = message(&app.composer_html(), &app.composer_mentions());
        let draft = load(&sent);
        let mut loaded = self::app();
        loaded.load_draft(load(&sent));
        (loaded, draft)
    }
    /// My message, with a reply quote and a table, edited in chat `chat` over the draft `brouillon`.
    fn editing() -> App {
        let mut app = app();
        *app.me = "8:orgid:me".into();
        let mine: Message = serde_json::from_value(serde_json::json!({
            "id": "mine",
            "from": "https://x/contacts/8:orgid:me",
            "messagetype": "RichText/Html",
            "content": r#"<blockquote itemtype="http://schema.skype.com/Reply"><p itemprop="preview">question</p></blockquote><p>réponse</p><table><tr><td>case</td></tr></table>"#,
        }))
        .unwrap();
        let _ = app.update(ConversationMsg::FeedLoaded {
            generation: app.generation,
            selection: Selection::Chat("chat".into()),
            append: false,
            result: Ok(MessagesPage {
                messages: vec![mine],
                ..Default::default()
            }),
        });
        let _ = app.update(Msg::DraftChanged("brouillon".into()));
        let _ = app.update(Msg::Edit("mine".into()));
        app
    }
    /// Teams' answer to saving the draft as it is now.
    fn saved(app: &App, id: &str, result: Result<(), String>) -> Msg {
        Msg::EditSaved {
            generation: app.generation,
            selection: Selection::Chat("chat".into()),
            id: id.into(),
            html: app.edited_html().unwrap(),
            result,
        }
    }
    #[test]
    fn an_edit_puts_back_what_its_draft_cannot_hold_and_then_the_draft_it_put_aside() {
        let mut app = editing();
        assert_eq!(app.conv.draft(), "réponse");
        let _ = app.update(Msg::DraftChanged("réponse corrigée".into()));
        let html = app.edited_html().unwrap();
        assert!(html.starts_with("<blockquote"), "{html}");
        assert!(html.contains("réponse corrigée"), "{html}");
        assert!(html.ends_with("</table>"), "{html}");
        let _ = app.update(Msg::CancelEdit);
        assert_eq!(app.conv.editing(), None);
        assert_eq!(app.conv.draft(), "brouillon");
        assert!(app.composer.edited.is_none());
    }
    #[test]
    fn an_edit_ends_only_when_teams_accepts_its_own_save_of_the_current_draft() {
        let mut app = editing();
        let _ = app.update(Msg::DraftChanged("réponse corrigée".into()));
        // Another message deleted meanwhile.
        let _ = app.update(ConversationMsg::Mutated {
            generation: app.generation,
            selection: Selection::Chat("chat".into()),
            result: Ok(()),
        });
        let _ = app.update(saved(&app, "mine", Err("refusé".into())));
        let _ = app.update(saved(&app, "other", Ok(())));
        let before_typing = saved(&app, "mine", Ok(()));
        let _ = app.update(Msg::DraftChanged("réponse corrigée encore".into()));
        let _ = app.update(before_typing);
        assert_eq!(app.conv.editing().map(String::as_str), Some("mine"));
        assert_eq!(app.conv.draft(), "réponse corrigée encore");
        let _ = app.update(saved(&app, "mine", Ok(())));
        assert_eq!(app.conv.editing(), None);
        assert_eq!(app.conv.draft(), "brouillon");
    }
    /// The demo app on a chat, with the id of one of my messages in it.
    async fn demo_chat() -> (Harness, String) {
        let mut h = Harness::demo();
        h.until("workspace", |a| !a.chats.is_empty()).await;
        h.send(Msg::Select(Selection::Chat(
            "19:alice_me@unq.gbl.spaces".into(),
        )));
        h.until("feed", |a| !a.conv.messages().is_empty()).await;
        let id = h
            .app
            .conv
            .messages()
            .iter()
            .find(|m| m.sender_mri() == mock::ME)
            .map(|m| m.id.clone())
            .unwrap();
        (h, id)
    }
    fn pasted_image() -> files::FileAction {
        files::FileAction::PasteImage {
            id: "collée".into(),
            png: b"png bytes".to_vec(),
            width: 320,
            height: 200,
        }
    }
    fn draft_with_the_image() -> Msg {
        Msg::RichDraftChanged(
            "corrigé \u{FFFC}".into(),
            vec![ComposerFormat {
                image: Some("collée".into()),
                ..format(8, 1)
            }],
        )
    }
    #[tokio::test]
    async fn sending_while_editing_saves_the_message_in_place_and_forgets_its_images() {
        let (mut h, id) = demo_chat().await;
        let count = h.app.conv.messages().len();
        let paste = |h: &mut Harness| h.file(pasted_image());
        h.send(Msg::Edit(id.clone()));
        paste(&mut h);
        h.send(Msg::CancelEdit);
        h.send(Msg::Edit(id.clone()));
        assert!(!h.app.has_attachments(), "kept after the cancel");
        paste(&mut h);
        h.send(draft_with_the_image());
        h.until("upload", |a| a.pending_views().iter().all(|p| p.ready))
            .await;
        h.send(Msg::Send);
        h.until("saved", |a| a.conv.editing().is_none()).await;
        h.until("feed with the edit", |a| {
            a.conv.messages().iter().any(|m| {
                m.id == id && m.content.contains("corrigé") && m.content.contains("AMSImage")
            })
        })
        .await;
        assert_eq!(h.app.conv.messages().len(), count);
        assert_eq!(h.app.error, None);
        h.send(Msg::Edit(id));
        assert!(!h.app.has_attachments(), "kept after the save");
    }
    #[tokio::test]
    async fn an_edit_with_a_failed_image_saves_once_the_image_is_taken_out_of_its_text() {
        let (mut h, id) = demo_chat().await;
        h.send(Msg::Edit(id.clone()));
        // Pasted without starting its upload, which then fails.
        let _ = h.app.update(Msg::Files(FilesMsg::Action(pasted_image())));
        let _ = h.app.update(Msg::Files(FilesMsg::Uploaded(
            h.app.generation,
            "collée".into(),
            Err("refusé".into()),
        )));
        h.send(draft_with_the_image());
        h.send(Msg::Send);
        assert!(h.app.notice.is_some());
        assert_eq!(h.app.conv.editing(), Some(&id));
        h.send(Msg::RichDraftChanged("corrigé".into(), vec![]));
        h.send(Msg::Send);
        h.until("saved", |a| a.conv.editing().is_none()).await;
        h.until("feed with the edit", |a| {
            a.conv
                .messages()
                .iter()
                .any(|m| m.id == id && m.content.contains("corrigé"))
        })
        .await;
    }
    #[test]
    fn composer_output_loads_back_to_the_same_draft() {
        let mut app = app();
        let text = "Titre\nun gras et lien\n\npuce\npuce 2\nnum\nfn main() {\n    x\n}\ncité\n────────\nfin centrée";
        let line = |n: usize| {
            text.split('\n')
                .take(n)
                .map(|l| l.encode_utf16().count() as u32 + 1)
                .sum::<u32>()
        };
        app.rich_draft_changed(
            text.into(),
            vec![
                ComposerFormat {
                    block: ComposerBlock::Heading1,
                    ..format(0, 6)
                },
                ComposerFormat {
                    bold: true,
                    ..format(line(1) + 3, 4)
                },
                ComposerFormat {
                    link: Some("https://example.com/".into()),
                    color: Some("#123456".into()),
                    ..format(line(1) + 11, 4)
                },
                ComposerFormat {
                    block: ComposerBlock::Bullet,
                    ..format(line(3), line(5) - line(3))
                },
                ComposerFormat {
                    block: ComposerBlock::Numbered,
                    indent: 1,
                    ..format(line(5), line(6) - line(5))
                },
                ComposerFormat {
                    block: ComposerBlock::CodeBlock,
                    ..format(line(6), line(9) - line(6))
                },
                ComposerFormat {
                    block: ComposerBlock::Quote,
                    ..format(line(9), line(10) - line(9))
                },
                ComposerFormat {
                    block: ComposerBlock::Rule,
                    ..format(line(10), line(11) - line(10))
                },
                ComposerFormat {
                    alignment: "center".into(),
                    ..format(line(11), 11)
                },
            ],
        );
        let (loaded, draft) = reloaded(&app);
        assert_eq!(draft.text, text);
        assert_eq!(loaded.conv.draft(), text);
        assert_eq!(loaded.composer_html(), app.composer_html());
        assert!(draft.posted.before.is_empty() && draft.posted.after.is_empty());
    }
    #[test]
    fn mentions_and_emoji_load_as_tokens() {
        let mut app = app();
        app.replace_draft(
            0,
            0,
            "@Alice & Bob ".into(),
            Some("8:orgid:alice".into()),
            None,
        );
        let emoji = crate::emoji::categories()[0].items[0].clone();
        app.replace_draft(
            app.conv.cursor(),
            0,
            emoji.text.clone(),
            None,
            Some(emoji.id.clone()),
        );
        let (loaded, _) = reloaded(&app);
        assert_eq!(loaded.conv.draft(), app.conv.draft().trim_end());
        assert_eq!(loaded.composer_html(), app.composer_html());
        assert_eq!(loaded.composer_mentions(), app.composer_mentions());
        assert_eq!(loaded.mention_ranges(), app.mention_ranges());
    }
    #[test]
    fn teams_message_keeps_reply_quote_images_and_whitespace_as_a_browser_shows_it() {
        let draft = load(&message(
            concat!(
                r#"<blockquote itemscope itemtype="http://schema.skype.com/Reply" itemid="1"><strong itemprop="mri">Vous</strong><p itemprop="preview">Avant</p></blockquote>"#,
                "<div>\n  <p>Salut   <span itemscope itemtype=\"http://schema.skype.com/Mention\" itemid=\"0\">Claire</span> ,</p>",
                r#"<p><img src="https://example.com/a.png"><span style="color:#f00">rouge</span><br></p>"#,
                r#"<p><br></p><ul><li>a<ul><li>b</li></ul></li></ul><table><tr><td>x</td></tr></table></div>"#,
            ),
            r#"[{"itemid":"0","mri":"8:orgid:claire","mentionType":"person"}]"#,
        ));
        assert_eq!(draft.text, "Salut @Claire ,\n\u{fffc}rouge\n\na\nb");
        assert!(
            draft.posted.before.starts_with("<blockquote") && draft.posted.before.contains("Avant")
        );
        assert!(draft.posted.after.starts_with("<table>"));
        assert_eq!(draft.tokens.len(), 1);
        let red = draft.formats.iter().find(|f| f.color.is_some()).unwrap();
        assert_eq!(
            (red.start, red.length, red.color.as_deref()),
            (17, 5, Some("#ff0000"))
        );
        let blocks: Vec<_> = draft
            .formats
            .iter()
            .map(|f| (f.start, f.block, f.indent))
            .collect();
        assert_eq!(
            blocks,
            [
                (16, ComposerBlock::Paragraph, 0),
                (17, ComposerBlock::Paragraph, 0),
                (24, ComposerBlock::Bullet, 0),
                (26, ComposerBlock::Bullet, 1)
            ]
        );
    }
    #[test]
    fn images_stay_where_they_were_and_go_back_as_posted() {
        let img = |n: u8| {
            format!(
                r#"<img src="https://eu-api.asm.skype.com/v1/objects/{n}/views/imgo" width="10" height="20" itemtype="http://schema.skype.com/AMSImage" id="x_{n}">"#
            )
        };
        let mut app = app();
        app.load_draft(load(&message(
            &format!("<p>avant {}{} après</p>", img(1), img(2)),
            "",
        )));
        assert_eq!(app.conv.draft(), "avant \u{fffc}\u{fffc} après");
        let images: Vec<_> = app
            .draft_formats()
            .into_iter()
            .filter_map(|f| f.image.map(|i| (f.start, f.length, i)))
            .collect();
        assert_eq!(images.len(), 2);
        assert_eq!(
            (images[0].0, images[0].1, images[1].0, images[1].1),
            (6, 1, 7, 1)
        );
        let html = scraper::Html::parse_fragment(&app.composer_html())
            .root_element()
            .html();
        let posted = |n| {
            scraper::Html::parse_fragment(&img(n))
                .root_element()
                .html()
                .replace("<html>", "")
                .replace("</html>", "")
        };
        assert!(
            html.contains(&format!("avant {}{} après", posted(1), posted(2))),
            "{html}"
        );
    }
    #[test]
    fn mentions_that_are_not_people_stay_text() {
        let draft = load(&message(
            r#"<p><span itemtype="http://schema.skype.com/Mention" itemid="0">Équipe</span></p>"#,
            r#"[{"itemid":0,"mri":"tag","mentionType":"tag"}]"#,
        ));
        assert_eq!(draft.text, "Équipe");
        assert!(draft.tokens.is_empty());
    }
    #[test]
    fn plain_text_messages_keep_their_lines() {
        let mut sent = message("a <b>\n\nc", "");
        sent.messagetype = "Text".into();
        let draft = load(&sent);
        assert_eq!(draft.text, "a <b>\n\nc");
        assert!(draft.formats.is_empty());
    }
}

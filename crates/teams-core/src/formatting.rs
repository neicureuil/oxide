//! Native rich-text runs, expressed without allowing arbitrary outgoing HTML.
use super::composer::byte_offset;
use super::*;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, uniffi::Enum)]
pub enum ComposerBlock {
    #[default]
    Paragraph,
    Heading1,
    Heading2,
    Heading3,
    Bullet,
    Numbered,
    Quote,
    CodeBlock,
    Rule,
}
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, uniffi::Record)]
pub struct ComposerFormat {
    pub start: u32,
    pub length: u32,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub strike: bool,
    pub code: bool,
    pub color: Option<String>,
    pub highlight: Option<String>,
    pub size: u32,
    pub link: Option<String>,
    pub block: ComposerBlock,
    pub indent: u32,
    pub alignment: String,
    /// A pasted image, by its draft attachment id: the format covers the one U+FFFC character standing for it.
    pub image: Option<String>,
}
/// The character a pasted image takes in the draft, as in AppKit text.
pub(crate) const IMAGE_PLACEHOLDER: char = '\u{FFFC}';
fn is_hex_color(value: &str) -> bool {
    value.len() == 7 && value.starts_with('#') && value[1..].bytes().all(|b| b.is_ascii_hexdigit())
}
impl App {
    pub(super) fn format_inserted(&mut self, start: u32, length: u32, mut style: ComposerFormat) {
        let end = start + length;
        let mut formats = Vec::new();
        for f in self.draft_formats() {
            let last = f.start + f.length;
            if f.start < start {
                let mut before = f.clone();
                before.length = last.min(start) - before.start;
                formats.push(before);
            }
            if last > end {
                let mut after = f;
                after.start = after.start.max(end);
                after.length = last - after.start;
                formats.push(after);
            }
        }
        style.start = start;
        style.length = length;
        formats.push(style);
        formats.sort_by_key(|f| f.start);
        self.rich_draft_changed(self.conv.draft().to_owned(), formats);
    }
    pub(super) fn composer_rich_html(&self) -> String {
        let mut html = String::new();
        let mut offset = 0;
        let mut group: Option<(ComposerBlock, String)> = None;
        for line in self.conv.draft().split('\n') {
            let style = self.style_at(offset);
            let block = style.map(|s| s.block).unwrap_or_default();
            let mut css = String::new();
            if let Some(s) = style {
                if s.indent > 0 {
                    css.push_str(&format!("margin-left:{}px;", s.indent * 24));
                }
                if matches!(s.alignment.as_str(), "center" | "right") {
                    css.push_str(&format!("text-align:{};", s.alignment));
                }
            }
            let attributes = if css.is_empty() {
                String::new()
            } else {
                format!(" style=\"{css}\"")
            };
            let grouped = matches!(
                block,
                ComposerBlock::Bullet | ComposerBlock::Numbered | ComposerBlock::CodeBlock
            );
            let next = grouped.then(|| (block, attributes.clone()));
            let continuing = next.is_some() && group == next;
            if group != next {
                if let Some((previous, _)) = group.take() {
                    html.push_str(close_group(previous));
                }
                if let Some((kind, attr)) = &next {
                    html.push_str(&match kind {
                        ComposerBlock::Bullet => format!("<ul{attr}>"),
                        ComposerBlock::Numbered => format!("<ol{attr}>"),
                        _ => format!("<pre{attr}><code>"),
                    });
                }
                group = next;
            }
            let content = self.composer_inline_html(offset, offset + line.len());
            match block {
                ComposerBlock::Bullet | ComposerBlock::Numbered => {
                    html.push_str(&format!("<li>{content}</li>"))
                }
                ComposerBlock::CodeBlock => {
                    if continuing {
                        html.push('\n');
                    }
                    html.push_str(&content);
                }
                ComposerBlock::Rule => html.push_str("<hr>"),
                ComposerBlock::Quote => {
                    html.push_str(&format!("<blockquote{attributes}>{content}</blockquote>"))
                }
                _ => {
                    let tag = match block {
                        ComposerBlock::Heading1 => "h1",
                        ComposerBlock::Heading2 => "h2",
                        ComposerBlock::Heading3 => "h3",
                        _ => "p",
                    };
                    html.push_str(&format!(
                        "<{tag}{attributes}>{}</{tag}>",
                        if content.is_empty() { "<br>" } else { &content }
                    ));
                }
            }
            offset += line.len() + 1;
        }
        if let Some((kind, _)) = group {
            html.push_str(close_group(kind));
        }
        html
    }
    pub(super) fn draft_formats(&self) -> Vec<ComposerFormat> {
        self.composer
            .formats
            .get(&self.draft_key())
            .cloned()
            .unwrap_or_default()
    }
    pub(super) fn rich_draft_changed(&mut self, value: String, formats: Vec<ComposerFormat>) {
        self.track_draft_edit(&value);
        self.conv.draft_typed(value);
        let mut end = 0;
        let formats = formats
            .into_iter()
            .filter_map(|mut f| {
                let last = f.start.checked_add(f.length)?;
                if f.length == 0
                    || f.start < end
                    || byte_offset(self.conv.draft(), f.start).is_none()
                    || byte_offset(self.conv.draft(), last).is_none()
                {
                    return None;
                }
                end = last;
                let placeholder = byte_offset(self.conv.draft(), f.start)
                    .is_some_and(|b| self.conv.draft()[b..].starts_with(IMAGE_PLACEHOLDER));
                f.image = f.image.filter(|_| f.length == 1 && placeholder);
                f.color = f.color.filter(|c| is_hex_color(c));
                f.highlight = f.highlight.filter(|c| is_hex_color(c));
                f.link = f.link.and_then(|url| crate::html::safe_link(&url));
                f.size = if f.size == 0 { 0 } else { f.size.clamp(10, 36) };
                f.indent = f.indent.min(8);
                if !matches!(f.alignment.as_str(), "left" | "center" | "right") {
                    f.alignment.clear();
                }
                Some(f)
            })
            .take(10000)
            .collect();
        let key = self.draft_key();
        self.composer.formats.insert(key, formats);
    }
    pub(super) fn shift_formats(&mut self, start: u32, end: u32, replacement: u32) {
        let key = self.draft_key();
        let Some(formats) = self.composer.formats.get_mut(&key) else {
            return;
        };
        let inherited = formats
            .iter()
            .find(|f| f.start <= start && f.start + f.length >= start)
            .cloned()
            .map(|mut style| {
                style.image = None;
                style
            });
        let mut shifted = Vec::new();
        for f in formats.drain(..) {
            let last = f.start + f.length;
            if f.start < start {
                let mut before = f.clone();
                before.length = last.min(start) - f.start;
                shifted.push(before);
            }
            if last > end {
                let suffix_start = f.start.max(end);
                let mut after = f;
                after.start = start + replacement + suffix_start - end;
                after.length = last - suffix_start;
                shifted.push(after);
            }
        }
        if replacement > 0
            && let Some(mut style) = inherited
        {
            style.start = start;
            style.length = replacement;
            shifted.push(style);
        }
        shifted.sort_by_key(|f| f.start);
        *formats = shifted;
    }
    /// Ids of the pasted images still in the draft.
    pub(crate) fn draft_image_ids(&self) -> Vec<String> {
        self.composer
            .formats
            .get(&self.draft_key())
            .into_iter()
            .flatten()
            .filter_map(|f| f.image.clone())
            .collect()
    }
    pub(super) fn style_at(&self, byte: usize) -> Option<&ComposerFormat> {
        let position = self.conv.draft()[..byte].encode_utf16().count() as u32;
        self.composer
            .formats
            .get(&self.draft_key())?
            .iter()
            .find(|f| f.start <= position && position < f.start + f.length)
    }
    pub(super) fn styled_text(&self, start: usize, end: usize) -> String {
        let mut output = String::new();
        let mut cursor = start;
        while cursor < end {
            let style = self.style_at(cursor);
            let next = self
                .composer
                .formats
                .get(&self.draft_key())
                .into_iter()
                .flatten()
                .flat_map(|f| [f.start, f.start + f.length])
                .filter_map(|p| byte_offset(self.conv.draft(), p))
                .filter(|b| *b > cursor)
                .min()
                .unwrap_or(end)
                .min(end);
            match style.and_then(|s| s.image.as_deref()) {
                Some(id) => {
                    if let Some(image) = self.inline_image(id) {
                        output.push_str(&image.tag());
                    } else if let Some(posted) = self.posted_image(id) {
                        output.push_str(posted);
                    }
                }
                None => {
                    output.push_str(&styled(escape(&self.conv.draft()[cursor..next]), style))
                }
            }
            cursor = next;
        }
        output
    }
}
fn close_group(kind: ComposerBlock) -> &'static str {
    match kind {
        ComposerBlock::Bullet => "</ul>",
        ComposerBlock::Numbered => "</ol>",
        _ => "</code></pre>",
    }
}
pub(super) fn styled(mut html: String, style: Option<&ComposerFormat>) -> String {
    let Some(s) = style else {
        return html;
    };
    if s.bold {
        html = format!("<strong>{html}</strong>");
    }
    if s.italic {
        html = format!("<em>{html}</em>");
    }
    if s.underline {
        html = format!("<u>{html}</u>");
    }
    if s.strike {
        html = format!("<s>{html}</s>");
    }
    if s.code {
        html = format!("<code>{html}</code>");
    }
    let mut css = String::new();
    if let Some(c) = &s.color {
        css.push_str(&format!("color:{c};"));
    }
    if let Some(c) = &s.highlight {
        css.push_str(&format!("background-color:{c};"));
    }
    if s.size > 0 {
        css.push_str(&format!("font-size:{}px;", s.size));
    }
    if !css.is_empty() {
        html = format!("<span style=\"{css}\">{html}</span>");
    }
    if let Some(link) = &s.link {
        html = format!("<a href=\"{}\">{html}</a>", escape_attr(link));
    }
    html
}

#[cfg(test)]
mod tests {
    use super::*;
    fn app() -> App {
        let mut app = App::empty();
        *app.selected = Some(Selection::Chat("chat".into()));
        app
    }
    #[test]
    fn rich_runs_escape_text_preserve_styles_and_reject_unsafe_attributes() {
        let mut app = app();
        app.rich_draft_changed(
            "😀 <texte>".into(),
            vec![ComposerFormat {
                start: 3,
                length: 7,
                bold: true,
                italic: true,
                underline: true,
                strike: true,
                color: Some("#123456".into()),
                highlight: Some("url(https://evil.test)".into()),
                link: Some("javascript:alert(1)".into()),
                size: 999,
                ..Default::default()
            }],
        );
        let html = app.composer_html();
        assert!(html.contains("<strong>&lt;texte&gt;</strong>"));
        assert!(html.contains("<u>"));
        assert!(html.contains("<s>"));
        assert!(html.contains("color:#123456;font-size:36px"));
        assert!(!html.contains("javascript"));
        assert!(!html.contains("evil.test"));
        app.rich_draft_changed(
            "😀 test".into(),
            vec![ComposerFormat {
                start: 1,
                length: 1,
                bold: true,
                ..Default::default()
            }],
        );
        assert!(
            !app.composer_html().contains("<strong>"),
            "a surrogate pair cannot be split by a run"
        );
    }
    #[test]
    fn paragraphs_preserve_numbering_code_newlines_and_safe_links() {
        let mut app = app();
        let lines = [
            ("Titre", ComposerBlock::Heading1),
            ("Un", ComposerBlock::Numbered),
            ("Deux", ComposerBlock::Numbered),
            ("Citation", ComposerBlock::Quote),
            ("let x = 1;", ComposerBlock::CodeBlock),
            (" x < 2", ComposerBlock::CodeBlock),
        ];
        let mut offset = 0;
        let mut formats = Vec::new();
        for (text, block) in lines {
            let length = text.encode_utf16().count() as u32 + 1;
            formats.push(ComposerFormat {
                start: offset,
                length,
                block,
                ..Default::default()
            });
            offset += length;
        }
        formats.last_mut().unwrap().length -= 1;
        app.rich_draft_changed(
            lines.iter().map(|(s, _)| *s).collect::<Vec<_>>().join("\n"),
            formats,
        );
        let html = app.composer_html();
        assert!(html.starts_with("<h1>Titre</h1><ol><li>Un</li><li>Deux</li></ol>"));
        assert!(html.contains("<blockquote>Citation</blockquote>"));
        assert!(html.ends_with("<pre><code>let x = 1;\n x &lt; 2</code></pre>"));
        let doc = crate::html::parse(&html);
        assert!(doc.text().contains("1. Un\n2. Deux"));
        assert!(doc.runs.iter().any(|r| r.size == 26 && r.bold));
    }
    #[test]
    fn inserting_emoji_inherits_style_and_preserves_suffix_and_per_thread_drafts() {
        let mut app = app();
        app.rich_draft_changed(
            "ABCD".into(),
            vec![ComposerFormat {
                length: 4,
                bold: true,
                ..Default::default()
            }],
        );
        let emoji = &crate::emoji::categories()[0].items[0];
        app.replace_draft(2, 0, emoji.text.clone(), None, Some(emoji.id.clone()));
        assert_eq!(app.conv.draft(), format!("AB{}CD", emoji.text));
        assert!(app.composer_html().contains(&format!(
            "<strong><ss type=\"{}\">{}</ss></strong>",
            emoji.id, emoji.text
        )));
        assert!(app.composer_html().contains("<strong>CD</strong>"));
        let html = app.composer_html();
        let _ = app.handle(ConversationMsg::OpenReply("post".into()));
        assert!(app.draft_formats().is_empty());
        let _ = app.handle(ConversationMsg::CloseReply);
        assert_eq!(app.composer_html(), html);
    }
    #[test]
    fn a_reply_quote_leaves_the_caret_on_a_plain_line_below_it() {
        let mut app = app();
        app.conv.draft_typed("Hi".into());
        let quote = ComposerFormat {
            block: ComposerBlock::Quote,
            ..Default::default()
        };
        let _ = app.update(Msg::ReplaceDraft {
            start: 0,
            length: 0,
            text: "Alice: ok".into(),
            mention: None,
            emoji: None,
            style: Some(quote),
        });
        let _ = app.update(Msg::ReplaceDraft {
            start: 9,
            length: 0,
            text: "\n".into(),
            mention: None,
            emoji: None,
            style: Some(ComposerFormat::default()),
        });
        assert_eq!(app.conv.draft(), "Alice: ok\nHi");
        assert_eq!(app.conv.cursor(), 10);
        let blocks: Vec<_> = app
            .draft_formats()
            .into_iter()
            .map(|f| (f.start, f.length, f.block))
            .collect();
        assert_eq!(
            blocks,
            [(0, 9, ComposerBlock::Quote), (9, 1, ComposerBlock::Paragraph)]
        );
    }
    #[test]
    fn text_typed_next_to_an_image_is_not_an_image() {
        let mut app = app();
        let image = ComposerFormat {
            start: 1,
            length: 1,
            bold: true,
            image: Some("img".into()),
            ..Default::default()
        };
        app.rich_draft_changed("a\u{FFFC}".into(), vec![image.clone()]);
        app.replace_draft(2, 0, "b".into(), None, None);
        assert_eq!(app.conv.draft(), "a\u{FFFC}b");
        let images: Vec<_> = app.draft_formats().into_iter().filter(|f| f.image.is_some()).collect();
        assert_eq!(images, [image]);
        assert_eq!(app.draft_image_ids(), ["img"]);
    }
}

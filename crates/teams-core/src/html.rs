//! Non-executing HTML model: Teams bodies become blocks drawn natively, plus flat runs for previews and search.
use crate::cards::{self, Card};
use chrono::{DateTime, Local, TimeZone};
use scraper::{ElementRef, Html, Node};
use teams_i18n::t;
use url::Url;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Run {
    pub text: String,
    pub link: Option<String>,
    pub bold: bool,
    pub italic: bool,
    pub code: bool,
    pub underline: bool,
    pub strike: bool,
    pub color: Option<String>,
    pub highlight: Option<String>,
    pub size: u32,
    /// An @mention of a person, team or channel.
    pub mention: bool,
}
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InlineImage {
    pub src: String,
    pub alt: String,
    /// The `width` and `height` attributes Teams writes: the size to reserve before loading.
    pub width: Option<u32>,
    pub height: Option<u32>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Block {
    /// Paragraphs and line breaks, as one flow of styled runs.
    Text {
        runs: Vec<Run>,
    },
    List {
        ordered: bool,
        start: u32,
        items: Vec<Vec<Block>>,
    },
    /// A quoted reply to another message (`schema.skype.com/Reply`), or a plain quote.
    Quote {
        author: Option<String>,
        time: Option<String>,
        message_id: Option<String>,
        body: Vec<Block>,
    },
    Code {
        language: Option<String>,
        text: String,
    },
    Table {
        header: bool,
        rows: Vec<Vec<Vec<Run>>>,
    },
    Rule,
    /// An inline image, where it sits in the body (the web client renders the HTML in order).
    Image(InlineImage),
    Card(Card),
}
#[derive(Debug, Clone, Default)]
pub struct Document {
    pub blocks: Vec<Block>,
    /// The blocks as flat text, without quoted replies: what previews and search use.
    pub runs: Vec<Run>,
    pub images: Vec<InlineImage>,
}
#[derive(Clone, Default)]
struct Style {
    link: Option<String>,
    bold: bool,
    italic: bool,
    code: bool,
    underline: bool,
    strike: bool,
    mention: bool,
    color: Option<String>,
    highlight: Option<String>,
    size: u32,
}
/// One container's content: finished blocks and the paragraph flow being built.
#[derive(Default)]
struct Flow {
    blocks: Vec<Block>,
    runs: Vec<Run>,
}
impl Flow {
    fn newline(&mut self) {
        line_break(&mut self.runs);
    }
    fn append(&mut self, value: &str, style: &Style) {
        let mut text = String::new();
        let mut whitespace = self
            .runs
            .last()
            .is_none_or(|r| r.text.ends_with(char::is_whitespace));
        for ch in value.chars() {
            if ch.is_whitespace() {
                if !whitespace {
                    text.push(' ');
                }
                whitespace = true;
            } else {
                text.push(ch);
                whitespace = false;
            }
        }
        if text.is_empty() {
            return;
        }
        if style.link.is_some() || style.code || style.mention {
            self.push(text, style, style.link.clone());
            return;
        }
        let finder = linkify::LinkFinder::new();
        let mut offset = 0;
        for link in finder.links(&text) {
            self.push(text[offset..link.start()].into(), style, None);
            let href = link.as_str();
            let href = if link.kind() == &linkify::LinkKind::Email {
                format!("mailto:{href}")
            } else if href.starts_with("www.") {
                format!("https://{href}")
            } else {
                href.to_owned()
            };
            self.push(link.as_str().into(), style, safe_link(&href));
            offset = link.end();
        }
        self.push(text[offset..].into(), style, None);
    }
    fn push(&mut self, text: String, style: &Style, link: Option<String>) {
        if !text.is_empty() {
            self.runs.push(Run {
                text,
                link,
                bold: style.bold,
                italic: style.italic,
                code: style.code,
                underline: style.underline,
                strike: style.strike,
                color: style.color.clone(),
                highlight: style.highlight.clone(),
                size: style.size,
                mention: style.mention,
            });
        }
    }
    /// Closes the current paragraph flow, dropping blank lines around it.
    fn flush(&mut self) {
        let mut runs = std::mem::take(&mut self.runs);
        while runs.last().is_some_and(|r| r.text.trim().is_empty()) {
            runs.pop();
        }
        if let Some(last) = runs.last_mut() {
            let trimmed = last.text.trim_end().len();
            last.text.truncate(trimmed);
        }
        let start = runs
            .iter()
            .position(|r| !r.text.trim().is_empty())
            .unwrap_or(runs.len());
        runs.drain(..start);
        if !runs.is_empty() {
            self.blocks.push(Block::Text { runs });
        }
    }
    fn block(&mut self, block: Block) {
        self.flush();
        self.blocks.push(block);
    }
    fn finish(mut self) -> Vec<Block> {
        self.flush();
        self.blocks
    }
}
/// Schemes that can be opened by an explicit click; never execute HTML or JS.
pub fn safe_link(value: &str) -> Option<String> {
    let url = Url::parse(value.trim()).ok()?;
    let allowed = match url.scheme() {
        "http" | "https" => {
            url.host_str().is_some() && url.username().is_empty() && url.password().is_none()
        }
        "mailto" => !url.path().is_empty(),
        _ => false,
    };
    allowed.then(|| url.to_string())
}
pub fn parse(html: &str) -> Document {
    let tree = Html::parse_fragment(html);
    let mut images = Vec::new();
    let mut flow = Flow::default();
    walk(
        tree.root_element(),
        &Style::default(),
        &mut flow,
        &mut images,
    );
    Document::new(flow.finish(), images)
}
/// A plain-text body (`messagetype` `Text`): line breaks kept, links detected.
pub fn plain(text: &str) -> Document {
    let mut flow = Flow::default();
    for (i, line) in text.lines().enumerate() {
        if i > 0 {
            flow.push("\n".into(), &Style::default(), None);
        }
        flow.append(line, &Style::default());
    }
    Document::new(flow.finish(), vec![])
}
impl Document {
    pub fn new(blocks: Vec<Block>, images: Vec<InlineImage>) -> Self {
        let mut runs = Vec::new();
        flatten(&blocks, &mut runs);
        Self {
            blocks,
            runs,
            images,
        }
    }
    pub fn text(&self) -> String {
        self.runs
            .iter()
            .map(|r| r.text.as_str())
            .collect::<String>()
            .trim()
            .to_owned()
    }
}
fn line_break(runs: &mut Vec<Run>) {
    if runs.last().is_some_and(|r| !r.text.ends_with('\n')) {
        runs.push(Run {
            text: "\n".into(),
            ..Default::default()
        });
    }
}
fn flatten(blocks: &[Block], runs: &mut Vec<Run>) {
    for block in blocks {
        match block {
            Block::Text { runs: text } => {
                line_break(runs);
                runs.extend(text.iter().cloned());
            }
            Block::List {
                ordered,
                start,
                items,
            } => {
                for (i, item) in items.iter().enumerate() {
                    line_break(runs);
                    runs.push(Run {
                        text: if *ordered {
                            format!("{}. ", *start as usize + i)
                        } else {
                            "• ".into()
                        },
                        ..Default::default()
                    });
                    let mut inner = Vec::new();
                    flatten(item, &mut inner);
                    runs.extend(inner);
                }
            }
            // A reply only points at another message: its text is not this one's.
            Block::Quote {
                author: Some(_), ..
            }
            | Block::Quote {
                message_id: Some(_),
                ..
            } => {}
            Block::Quote { body, .. } => flatten(body, runs),
            Block::Code { text, .. } => {
                line_break(runs);
                runs.push(Run {
                    text: text.clone(),
                    code: true,
                    ..Default::default()
                });
            }
            Block::Table { rows, .. } => {
                for row in rows {
                    line_break(runs);
                    for (i, cell) in row.iter().enumerate() {
                        if i > 0 {
                            runs.push(Run {
                                text: " | ".into(),
                                ..Default::default()
                            });
                        }
                        runs.extend(cell.iter().cloned());
                    }
                }
            }
            Block::Rule | Block::Image(_) => {}
            Block::Card(card) => {
                line_break(runs);
                runs.extend(card.text_runs());
            }
        }
    }
}
fn css(element: ElementRef<'_>, style: &mut Style) {
    let Some(css) = element.value().attr("style") else {
        return;
    };
    for declaration in css.split(';') {
        let Some((key, value)) = declaration.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match key.trim().to_ascii_lowercase().as_str() {
            "color" => style.color = color(value).or(style.color.take()),
            "background-color" => style.highlight = color(value).or(style.highlight.take()),
            "font-weight" if matches!(value, "bold" | "600" | "700" | "800" | "900") => {
                style.bold = true
            }
            "font-style" if value == "italic" => style.italic = true,
            "text-decoration" | "text-decoration-line" => {
                style.underline |= value.contains("underline");
                style.strike |= value.contains("line-through");
            }
            "font-size" => {
                if let Some(size) = value.strip_suffix("px").and_then(|n| n.parse::<u32>().ok()) {
                    style.size = size.clamp(10, 36);
                }
            }
            _ => {}
        }
    }
}
fn itemtype(element: ElementRef<'_>) -> &str {
    element
        .value()
        .attr("itemtype")
        .and_then(|t| t.rsplit_once("schema.skype.com/"))
        .map_or("", |(_, t)| t)
}
fn walk(element: ElementRef<'_>, parent: &Style, flow: &mut Flow, images: &mut Vec<InlineImage>) {
    let tag = element.value().name();
    if matches!(
        tag,
        "script" | "style" | "iframe" | "object" | "noscript" | "head" | "legacyquote"
    ) {
        return;
    }
    let mut style = parent.clone();
    css(element, &mut style);
    match (tag, itemtype(element)) {
        (_, "Emoji" | "CustomEmoji") | ("emoji", _) => {
            flow.append(&emoji_text(element), &style);
            return;
        }
        (_, "Mention") | ("at", _) => style.mention = true,
        ("blockquote" | "quote", _) => {
            flow.block(quote(element, &style, images));
            return;
        }
        ("pre" | "codeblock", _) => {
            flow.block(code(element));
            return;
        }
        ("ul" | "ol", _) => {
            flow.block(list(element, &style, images));
            return;
        }
        ("table", _) => {
            flow.block(table(element, &style, images));
            return;
        }
        ("hr", _) => {
            flow.block(Block::Rule);
            return;
        }
        ("uriobject", _) => {
            let found = cards::from_uri_object(element);
            if !found.is_empty() {
                for card in found {
                    flow.block(Block::Card(card));
                }
                return;
            }
        }
        ("a", _) => style.link = element.value().attr("href").and_then(safe_link),
        ("b" | "strong", _) => style.bold = true,
        ("em" | "i", _) => style.italic = true,
        ("u", _) => style.underline = true,
        ("s" | "del" | "strike", _) => style.strike = true,
        ("h1" | "h2" | "h3" | "h4" | "h5" | "h6", _) => {
            flow.newline();
            style.bold = true;
            style.size = match tag {
                "h1" => 26,
                "h2" => 22,
                "h3" => 18,
                "h4" => 16,
                "h5" => 15,
                _ => 14,
            };
        }
        ("code", _) => style.code = true,
        ("br", _) => {
            flow.push("\n".into(), &style, None);
            return;
        }
        ("img", _) => {
            if let Some(image) = image(element, images) {
                flow.block(Block::Image(image));
            }
            return;
        }
        ("p" | "div", _) => flow.newline(),
        _ => {}
    }
    children(element, &style, flow, images);
    if matches!(tag, "p" | "div") || tag.len() == 2 && tag.starts_with('h') {
        flow.newline();
    }
}
fn children(
    element: ElementRef<'_>,
    style: &Style,
    flow: &mut Flow,
    images: &mut Vec<InlineImage>,
) {
    for child in element.children() {
        match child.value() {
            Node::Text(text) => flow.append(text, style),
            Node::Element(_) => {
                if let Some(el) = ElementRef::wrap(child) {
                    walk(el, style, flow, images);
                }
            }
            _ => {}
        }
    }
}
/// Records an image once; returns it the first time it is seen.
fn image(element: ElementRef<'_>, images: &mut Vec<InlineImage>) -> Option<InlineImage> {
    let attrs = element.value();
    let size = |key: &str| {
        attrs
            .attr(key)
            .and_then(|v| v.trim().trim_end_matches("px").parse::<f64>().ok())
            .filter(|v| *v >= 1.0)
            .map(|v| v.round() as u32)
    };
    if let Some(src) = image_src(element)
        && !images.iter().any(|i| i.src == src)
    {
        let image = InlineImage {
            src: src.to_owned(),
            alt: attrs
                .attr("alt")
                .filter(|s| !s.trim().is_empty())
                .unwrap_or(t("core.html.shared_image"))
                .to_owned(),
            width: size("width"),
            height: size("height"),
        };
        images.push(image.clone());
        return Some(image);
    }
    None
}
/// Where an `<img>` shows its picture from, which is also its key among a message's images.
pub(crate) fn image_src<'a>(element: ElementRef<'a>) -> Option<&'a str> {
    ["data-orig-src", "data-image-url", "src"]
        .into_iter()
        .filter_map(|key| element.value().attr(key))
        .find(|s| !s.trim().is_empty() && !s.starts_with("blob:"))
}
/// Teams emoji are images of a Unicode character: keep the character in the text.
fn emoji_text(element: ElementRef<'_>) -> String {
    let attrs = element.value();
    if let Some(alt) = attrs.attr("alt").map(str::trim).filter(|s| !s.is_empty()) {
        return alt.to_owned();
    }
    let inner: String = element.text().collect();
    if !inner.trim().is_empty() {
        return inner.trim().to_owned();
    }
    let id = attrs.attr("itemid").or(attrs.attr("id")).unwrap_or("");
    crate::emoji::reaction(id).0
}
fn quote(element: ElementRef<'_>, style: &Style, images: &mut Vec<InlineImage>) -> Block {
    let attrs = element.value();
    let reply = itemtype(element) == "Reply";
    let mut author = attrs
        .attr("authorname")
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    let mut time = attrs
        .attr("timestamp")
        .and_then(|t| t.parse::<i64>().ok())
        .and_then(stamp);
    let message_id = attrs
        .attr("itemid")
        .or(attrs.attr("messageid"))
        .filter(|_| reply || element.value().name() == "quote")
        .filter(|id| !id.is_empty() && id.bytes().all(|b| b.is_ascii_digit()))
        .map(str::to_owned);
    let mut body = Flow::default();
    for child in element.children() {
        match child.value() {
            Node::Text(text) => body.append(text, style),
            Node::Element(_) => {
                let Some(el) = ElementRef::wrap(child) else {
                    continue;
                };
                match el.value().attr("itemprop") {
                    Some("mri") => {
                        let name = el.text().collect::<String>().trim().to_owned();
                        if !name.is_empty() {
                            author = Some(name);
                        }
                    }
                    Some("time") => {
                        time = el
                            .value()
                            .attr("itemid")
                            .and_then(|t| t.parse::<i64>().ok())
                            .and_then(stamp)
                            .or(time);
                    }
                    _ => walk(el, style, &mut body, images),
                }
            }
            _ => {}
        }
    }
    Block::Quote {
        author,
        time,
        message_id,
        body: body.finish(),
    }
}
/// A quoted message's local time (Skype seconds or Teams milliseconds) as `16/10 14:05` (`10/16 14:05` in English).
fn stamp(epoch: i64) -> Option<String> {
    let millis = if epoch < 100_000_000_000 {
        epoch * 1000
    } else {
        epoch
    };
    let utc = DateTime::from_timestamp_millis(millis)?;
    let local = Local.from_utc_datetime(&utc.naive_utc());
    Some(local.format(t("core.html.m_d_h_m")).to_string())
}
fn code(element: ElementRef<'_>) -> Block {
    let language = std::iter::once(element)
        .chain(element.children().filter_map(ElementRef::wrap))
        .filter_map(|el| el.value().attr("class"))
        .flat_map(str::split_whitespace)
        .find_map(|class| {
            let name = class.strip_prefix("language-").unwrap_or(class);
            (!name.is_empty() && name != "code" && !name.starts_with("hljs"))
                .then(|| name.to_owned())
        })
        .or_else(|| {
            // Legacy `<codeblock class="JavaScript">`.
            (element.value().name() == "codeblock")
                .then(|| element.value().attr("class"))
                .flatten()
                .map(str::to_owned)
        })
        .filter(|l| !l.eq_ignore_ascii_case("plaintext") && !l.eq_ignore_ascii_case("text"));
    let mut text = String::new();
    verbatim(element, &mut text);
    let text = text.trim_matches('\n').trim_end().to_owned();
    Block::Code { language, text }
}
/// Code text as written: whitespace kept, `br` and block elements as line breaks.
fn verbatim(element: ElementRef<'_>, out: &mut String) {
    for child in element.children() {
        match child.value() {
            Node::Text(text) => out.push_str(text),
            Node::Element(_) => {
                let Some(el) = ElementRef::wrap(child) else {
                    continue;
                };
                match el.value().name() {
                    "br" => out.push('\n'),
                    "script" | "style" => {}
                    "p" | "div" | "li" => {
                        if !out.is_empty() && !out.ends_with('\n') {
                            out.push('\n');
                        }
                        verbatim(el, out);
                        if !out.ends_with('\n') {
                            out.push('\n');
                        }
                    }
                    _ => verbatim(el, out),
                }
            }
            _ => {}
        }
    }
}
fn list(element: ElementRef<'_>, style: &Style, images: &mut Vec<InlineImage>) -> Block {
    let ordered = element.value().name() == "ol";
    let start = element
        .value()
        .attr("start")
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or(1);
    let mut items: Vec<Vec<Block>> = Vec::new();
    for child in element.children().filter_map(ElementRef::wrap) {
        let mut item = Flow::default();
        if child.value().name() == "li" {
            let mut item_style = style.clone();
            css(child, &mut item_style);
            children(child, &item_style, &mut item, images);
        } else {
            // A nested list written beside its items instead of inside one.
            walk(child, style, &mut item, images);
            if let Some(last) = items.last_mut()
                && matches!(child.value().name(), "ul" | "ol")
            {
                last.extend(item.finish());
                continue;
            }
        }
        let blocks = item.finish();
        if !blocks.is_empty() {
            items.push(blocks);
        }
    }
    Block::List {
        ordered,
        start,
        items,
    }
}
fn table(element: ElementRef<'_>, style: &Style, images: &mut Vec<InlineImage>) -> Block {
    fn rows<'a>(element: ElementRef<'a>, out: &mut Vec<ElementRef<'a>>) {
        for child in element.children().filter_map(ElementRef::wrap) {
            match child.value().name() {
                "tr" => out.push(child),
                "thead" | "tbody" | "tfoot" => rows(child, out),
                _ => {}
            }
        }
    }
    let mut found = Vec::new();
    rows(element, &mut found);
    let mut header = false;
    let mut cells = Vec::new();
    for (i, row) in found.into_iter().enumerate() {
        let mut line = Vec::new();
        let mut all_th = true;
        for cell in row.children().filter_map(ElementRef::wrap) {
            let name = cell.value().name();
            if !matches!(name, "td" | "th") {
                continue;
            }
            all_th &= name == "th";
            let mut cell_style = style.clone();
            cell_style.bold |= name == "th";
            css(cell, &mut cell_style);
            let mut flow = Flow::default();
            children(cell, &cell_style, &mut flow, images);
            let mut runs = Vec::new();
            flatten(&flow.finish(), &mut runs);
            line.push(runs);
        }
        if line.is_empty() {
            continue;
        }
        if i == 0 {
            header = all_th;
        }
        cells.push(line);
    }
    Block::Table {
        header,
        rows: cells,
    }
}
/// `#rgb`, `#rrggbb` or `rgb(r, g, b)`, normalized to hex; anything else is dropped.
fn color(value: &str) -> Option<String> {
    let value = value.trim();
    if matches!(value.len(), 4 | 7)
        && value.starts_with('#')
        && value[1..].bytes().all(|b| b.is_ascii_hexdigit())
    {
        return Some(value.to_owned());
    }
    let inner = value
        .strip_prefix("rgb(")
        .or_else(|| value.strip_prefix("rgba("))?
        .strip_suffix(')')?;
    let parts: Vec<u8> = inner
        .split(',')
        .take(3)
        .map(|p| p.trim().parse().ok())
        .collect::<Option<_>>()?;
    (parts.len() == 3).then(|| format!("#{:02x}{:02x}{:02x}", parts[0], parts[1], parts[2]))
}
pub fn to_text(html: &str) -> String {
    parse(html).text()
}
/// [`to_text`] for previews shown on every diff: each distinct content is parsed once, not on every refresh.
pub fn to_text_cached(html: &str) -> String {
    use std::collections::HashMap;
    use std::hash::{BuildHasher, RandomState};
    use std::sync::{LazyLock, Mutex};
    /// Enough for every preview of a large account; cleared wholesale beyond that.
    const MAX: usize = 4096;
    static HASHER: LazyLock<RandomState> = LazyLock::new(RandomState::new);
    static CACHE: LazyLock<Mutex<HashMap<u64, String>>> = LazyLock::new(Default::default);
    // The text holds fallbacks in the current language.
    let key = HASHER.hash_one((teams_i18n::english(), html));
    if let Some(text) = CACHE.lock().unwrap().get(&key) {
        return text.clone();
    }
    let text = to_text(html);
    let mut cache = CACHE.lock().unwrap();
    if cache.len() >= MAX {
        cache.clear();
    }
    cache.insert(key, text.clone());
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn preserves_paragraphs_and_decodes_entities_once() {
        assert_eq!(
            to_text("<p>Hello <b>world</b></p><p>Line<br>break</p>"),
            "Hello world\nLine\nbreak"
        );
        assert_eq!(to_text("a &amp;lt; b &lt; c"), "a &lt; b < c");
        assert_eq!(to_text("2 < 3"), "2 < 3");
    }
    #[test]
    fn renders_nested_links_and_detects_plain_urls() {
        let doc = parse(
            "<p>Voir <a href='https://example.test/?a=1&amp;b=2'><strong>le site</strong></a> puis https://rust-lang.org.</p>",
        );
        assert!(doc.runs.iter().any(|r| r.text == "le site"
            && r.bold
            && r.link.as_deref() == Some("https://example.test/?a=1&b=2")));
        assert!(
            doc.runs
                .iter()
                .any(|r| r.link.as_deref() == Some("https://rust-lang.org/"))
        );
    }
    #[test]
    fn images_stay_where_they_are_in_the_body_with_their_declared_size() {
        let doc = parse(
            "<p><img src='https://x/a' width='432' height=\"111.5px\" itemtype='http://schema.skype.com/AMSImage'></p><p>Texte</p>",
        );
        assert!(matches!(
            &doc.blocks[..],
            [Block::Image(InlineImage { width: Some(432), height: Some(112), .. }), Block::Text { .. }]
        ));
        assert_eq!(doc.text(), "Texte");
    }
    #[test]
    fn extracts_original_image_and_ignores_scripts_and_unsafe_links() {
        let doc = parse(
            "<script>alert('x')</script><a href='javascript:evil()'>visible</a><img src='blob:expired' data-orig-src='https://eu-api.asm.skype.com/v1/objects/x/views/imgo' alt='Capture'>",
        );
        assert_eq!(doc.text(), "visible");
        assert!(doc.runs.iter().all(|r| r.link.is_none()));
        assert_eq!(doc.images[0].alt, "Capture");
        assert!(doc.images[0].src.starts_with("https://"));
    }
    #[test]
    fn reply_quotes_are_a_block_with_author_and_preview() {
        let doc = parse(
            r#"<blockquote itemscope itemtype="http://schema.skype.com/Reply" itemid="1700000000000">
<strong itemprop="mri" itemid="8:orgid:abc">Alice Martin</strong><span itemprop="time" itemid="1700000000000"></span>
<p itemprop="preview">Le build passe ?</p></blockquote><p>Oui, c'est vert.</p>"#,
        );
        let Block::Quote {
            author,
            time,
            message_id,
            body,
        } = &doc.blocks[0]
        else {
            panic!("{:?}", doc.blocks);
        };
        assert_eq!(author.as_deref(), Some("Alice Martin"));
        assert!(time.is_some());
        assert_eq!(message_id.as_deref(), Some("1700000000000"));
        assert!(matches!(&body[..], [Block::Text { runs }] if runs[0].text == "Le build passe ?"));
        // Previews show the reply, not what it quotes.
        assert_eq!(doc.text(), "Oui, c'est vert.");
    }
    #[test]
    fn skype_quotes_drop_their_legacy_brackets() {
        let doc = parse(
            r#"<quote authorname="Bob" timestamp="1700000000" messageid="1700000000123"><legacyquote>[16:00] Bob: </legacyquote>Salut<legacyquote>

&lt;&lt;&lt; </legacyquote></quote>Réponse"#,
        );
        let Block::Quote { author, body, .. } = &doc.blocks[0] else {
            panic!("{:?}", doc.blocks);
        };
        assert_eq!(author.as_deref(), Some("Bob"));
        assert!(matches!(&body[..], [Block::Text { runs }] if runs[0].text == "Salut"));
        assert_eq!(doc.text(), "Réponse");
    }
    #[test]
    fn code_blocks_keep_lines_and_language() {
        let doc = parse(
            "<p>Voici :</p><pre class=\"language-rust\"><code>fn main() {\n    let x = 1;\n}</code></pre>",
        );
        assert_eq!(
            doc.blocks[1],
            Block::Code {
                language: Some("rust".into()),
                text: "fn main() {\n    let x = 1;\n}".into()
            }
        );
        let legacy = parse(
            "<codeblock class=\"Python\"><code>a = 1<br>if a:<br>    print(a)</code></codeblock>",
        );
        assert_eq!(
            legacy.blocks[0],
            Block::Code {
                language: Some("Python".into()),
                text: "a = 1\nif a:\n    print(a)".into()
            }
        );
    }
    #[test]
    fn lists_nest_and_keep_their_first_line() {
        let doc = parse(
            "<ol start=\"3\"><li><p>Trois</p></li><li>Quatre<ul><li>a</li><li>b</li></ul></li></ol>",
        );
        let Block::List {
            ordered,
            start,
            items,
        } = &doc.blocks[0]
        else {
            panic!("{:?}", doc.blocks);
        };
        assert!(ordered);
        assert_eq!(*start, 3);
        assert!(
            matches!(&items[0][..], [Block::Text { runs }] if runs.len() == 1 && runs[0].text == "Trois")
        );
        assert!(
            matches!(&items[1][1], Block::List { ordered: false, items, .. } if items.len() == 2)
        );
        assert!(doc.text().starts_with("3. Trois\n4. Quatre"));
    }
    #[test]
    fn tables_keep_their_cells_apart() {
        let doc = parse(
            "<table><tbody><tr><th>Nom</th><th>État</th></tr><tr><td>API</td><td><b>OK</b></td></tr></tbody></table>",
        );
        let Block::Table { header, rows } = &doc.blocks[0] else {
            panic!("{:?}", doc.blocks);
        };
        assert!(header);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[1][0][0].text, "API");
        assert!(rows[1][1][0].bold);
        assert!(rows[0][0][0].bold);
    }
    #[test]
    fn emoji_stay_in_the_text_and_are_not_previews() {
        let doc = parse(
            r#"<p>Bravo <span class="animated-emoticon-20-clap" type="(clap)"><img itemscope itemtype="http://schema.skype.com/Emoji" itemid="clap" src="https://statics.teams.cdn.office.net/evergreen-assets/personal-expressions/v2/assets/emoticons/clap/default/20_f.png" alt="👏" width="20" height="20"></span> !</p><img itemtype="http://schema.skype.com/Emoji" itemid="1f44d_thumbsup" src="x.png">"#,
        );
        assert!(doc.images.is_empty());
        assert_eq!(doc.text(), "Bravo 👏 !\n👍");
    }
    #[test]
    fn mentions_and_rgb_colors_are_styled() {
        let doc = parse(
            r#"<p><span itemscope itemtype="http://schema.skype.com/Mention" itemid="0">Alice</span> <span style="color: rgb(255, 0, 16)">rouge</span></p>"#,
        );
        assert!(doc.runs.iter().any(|r| r.text == "Alice" && r.mention));
        assert!(
            doc.runs
                .iter()
                .any(|r| r.text == "rouge" && r.color.as_deref() == Some("#ff0010"))
        );
    }
    #[test]
    fn plain_text_bodies_are_not_parsed_as_html() {
        assert_eq!(
            plain("a <b> c\nhttps://x.test").text(),
            "a <b> c\nhttps://x.test"
        );
    }
}

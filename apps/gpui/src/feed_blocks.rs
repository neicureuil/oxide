//! A message body (`MessageRichText.swift`, `MessageBlocks.swift`): paragraphs, lists, quotes, code, tables, pictures
//! and cards; plus the pictures, files and link previews under a message (`PreviewImage`, `FileCard`, `LinkPreviewCard`).
use crate::feed::{ghost, outline};
use crate::icons::{Icon, icon};
use crate::oxide::{FONT, MONO, Oxide, tr};
use crate::theme::Theme;
use crate::widgets;
use gpui::{
    AnyElement, ClipboardItem, Context, Corner, FontStyle, FontWeight, Hsla, InteractiveText,
    ObjectFit, SharedString, StrikethroughStyle, StyledText, TextRun, UnderlineStyle, Window, div,
    img, prelude::*, px, rgb,
};
use std::sync::Arc;
use teams_core::{
    Action, BlockView, CardActionKind, CardColumnWidth, CardInputKind, CardInputValue,
    CardInputView, CardItemView, CardView, ImageStatus, ImageView, LinkPreviewView, OxideCore,
    RunView,
};

/// Who sent a message's pictures and when, for their full-size view.
#[derive(Clone, Debug, Default)]
pub struct Caption {
    pub sender: String,
    pub time: String,
}

/// What a block is drawn with: its message (for cards and ids), base size and colour.
#[derive(Clone)]
struct Ctx<'a> {
    message: &'a str,
    path: String,
    size: f32,
    color: Hsla,
    caption: Option<&'a Caption>,
}
impl Ctx<'_> {
    fn child(&self, part: impl std::fmt::Display) -> Self {
        Self {
            path: format!("{}/{part}", self.path),
            ..self.clone()
        }
    }
}

const BLOCK_SPACING: f32 = 8.;

/// A message body; `size` is the base for runs without their own (30 for emoji alone).
pub fn body(
    this: &mut Oxide,
    message: &str,
    blocks: &[BlockView],
    size: f32,
    caption: Option<&Caption>,
    window: &mut Window,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    let ctx = Ctx {
        message,
        path: message.to_owned(),
        size,
        color: this.theme.text,
        caption,
    };
    // Its paragraphs take part in the feed's selection, in order (`MessageBody`'s units).
    crate::feed_select::drawing_message(Some(message));
    let body = block_list(this, &ctx, blocks, BLOCK_SPACING, window, cx);
    crate::feed_select::drawing_message(None);
    body
}

fn block_list(
    this: &mut Oxide,
    ctx: &Ctx,
    blocks: &[BlockView],
    spacing: f32,
    window: &mut Window,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    let mut column = div().min_w_0().flex().flex_col().gap(px(spacing));
    for (ix, b) in blocks.iter().enumerate() {
        column = column.child(block(this, &ctx.child(ix), b, window, cx));
    }
    column.into_any_element()
}

/// Images drawn in the body, where the HTML puts them.
pub fn images(blocks: &[BlockView]) -> Vec<&ImageView> {
    blocks
        .iter()
        .flat_map(|b| match b {
            BlockView::Image { image } => vec![image],
            BlockView::List { items, .. } => items.iter().flat_map(|i| images(&i.blocks)).collect(),
            BlockView::Quote { body, .. } => images(body),
            _ => vec![],
        })
        .collect()
}

/// Whether a character is drawn as an emoji (`Character.isEmojiGlyph`).
fn is_emoji(grapheme: &str) -> bool {
    let mut chars = grapheme.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    let more = chars.next().is_some();
    let c = first as u32;
    let pictographic = matches!(c, 0x1F000..=0x1FAFF | 0x2600..=0x27BF | 0x2B00..=0x2BFF);
    pictographic
        || (more && (c >= 0x2000 || first.is_ascii_digit() || first == '#' || first == '*'))
}

/// One to three emoji alone are drawn large, like Teams.
pub fn emoji_only(blocks: &[BlockView]) -> bool {
    use unicode_segmentation::UnicodeSegmentation;
    let [BlockView::Text { runs }] = blocks else {
        return false;
    };
    let text: String = runs.iter().map(|r| r.text.as_str()).collect();
    let glyphs: Vec<&str> = text
        .graphemes(true)
        .filter(|g| !g.trim().is_empty())
        .collect();
    (1..=3).contains(&glyphs.len()) && glyphs.iter().all(|g| is_emoji(g))
}

/// The text of `blocks` on one line, cut at `limit` characters.
/// A body's text for copying, one line per paragraph, list item and table row (what Swift took from the
/// flat `runs`, which the feed no longer keeps).
pub fn body_text(blocks: &[BlockView]) -> String {
    fn runs(runs: &[teams_core::RunView]) -> String {
        runs.iter().map(|r| r.text.as_str()).collect()
    }
    fn lines(block: &BlockView, out: &mut Vec<String>) {
        match block {
            BlockView::Text { runs: r } => out.push(runs(r)),
            BlockView::List { items, .. } => items.iter().flat_map(|i| i.blocks.iter()).for_each(|b| lines(b, out)),
            BlockView::Quote { body, .. } => body.iter().for_each(|b| lines(b, out)),
            BlockView::Code { text, .. } => out.push(text.clone()),
            BlockView::Table { rows, .. } => out.extend(
                rows.iter().map(|r| r.cells.iter().map(|c| runs(&c.runs)).collect::<Vec<_>>().join("\t")),
            ),
            _ => {}
        }
    }
    let mut out = Vec::new();
    blocks.iter().for_each(|b| lines(b, &mut out));
    out.join("\n")
}

pub fn excerpt(blocks: &[BlockView], limit: usize) -> String {
    fn text(block: &BlockView) -> String {
        match block {
            BlockView::Text { runs } => runs.iter().map(|r| r.text.as_str()).collect(),
            BlockView::List { items, .. } => items
                .iter()
                .flat_map(|i| i.blocks.iter())
                .map(text)
                .collect::<Vec<_>>()
                .join(" "),
            BlockView::Quote { body, .. } => body.iter().map(text).collect::<Vec<_>>().join(" "),
            BlockView::Code { text, .. } => text.clone(),
            BlockView::Table { rows, .. } => rows
                .iter()
                .flat_map(|r| r.cells.iter())
                .map(|c| c.runs.iter().map(|r| r.text.as_str()).collect::<String>())
                .collect::<Vec<_>>()
                .join(" "),
            _ => String::new(),
        }
    }
    let flat = blocks.iter().map(text).collect::<Vec<_>>().join(" ");
    let flat = flat
        .split(['\n', '\r'])
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if flat.chars().count() > limit {
        format!("{}…", flat.chars().take(limit).collect::<String>())
    } else {
        flat
    }
}

fn block(
    this: &mut Oxide,
    ctx: &Ctx,
    block: &BlockView,
    window: &mut Window,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    let theme = this.theme;
    match block {
        BlockView::Text { runs } => {
            rich_text(&ctx.path, runs, ctx.size, ctx.color, &theme, &this.core)
        }
        BlockView::List {
            ordered,
            start,
            items,
        } => {
            let mut column = div().min_w_0().flex().flex_col().gap(px(4.));
            for (ix, item) in items.iter().enumerate() {
                let mark = if *ordered {
                    format!("{}.", *start as usize + ix)
                } else {
                    "•".into()
                };
                column = column.child(
                    div()
                        .min_w_0()
                        .flex()
                        .items_start()
                        .gap(px(6.))
                        .pl(px(2.))
                        .child(
                            div()
                                .flex_none()
                                .w(px(14.))
                                .flex()
                                .justify_end()
                                .text_size(px(ctx.size))
                                .line_height(px(line_height(ctx.size)))
                                .text_color(theme.text_soft)
                                .child(mark),
                        )
                        .child(div().flex_1().min_w_0().child(block_list(
                            this,
                            &ctx.child(format!("i{ix}")),
                            &item.blocks,
                            4.,
                            window,
                            cx,
                        ))),
                );
            }
            column.into_any_element()
        }
        BlockView::Quote {
            author,
            time,
            message_id,
            body,
        } => {
            let inner = Ctx {
                size: 13.,
                color: theme.text_soft,
                ..ctx.child("q")
            };
            let reply = message_id.is_some() || author.is_some();
            let mut column = div()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(3.))
                .pl(px(12.))
                .pr(px(10.))
                .py(px(6.))
                .rounded(px(6.))
                .bg(theme.input)
                .border_l(px(3.))
                .border_color(theme.accent);
            if author.is_some() || time.is_some() {
                column = column.child(
                    div()
                        .flex()
                        .items_baseline()
                        .gap(px(6.))
                        .children(author.clone().map(|a| {
                            div()
                                .text_size(px(12.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme.text)
                                .child(a)
                        }))
                        .children(
                            time.clone()
                                .map(|t| div().text_size(px(11.)).text_color(theme.muted).child(t)),
                        ),
                );
            }
            if reply {
                // A reply shows the start of the message it answers, as Teams does, not the whole of it.
                let run = RunView {
                    text: excerpt(body, 280),
                    link: None,
                    bold: false,
                    italic: false,
                    code: false,
                    underline: false,
                    strike: false,
                    color: None,
                    highlight: None,
                    size: 0,
                    mention: false,
                };
                column = column.child(rich_text(
                    &inner.path,
                    &[run],
                    13.,
                    theme.text_soft,
                    &theme,
                    &this.core,
                ));
            } else {
                column = column.child(block_list(this, &inner, body, BLOCK_SPACING, window, cx));
            }
            column.into_any_element()
        }
        BlockView::Code { language, text } => code(this, &ctx.path, language.as_deref(), text, cx),
        BlockView::Table { header, rows } => {
            let columns = rows.iter().map(|r| r.cells.len()).max().unwrap_or(0);
            if columns == 0 {
                return div().into_any_element();
            }
            let mut table = div()
                .min_w_0()
                .flex()
                .flex_col()
                .border_1()
                .border_color(theme.border);
            for (r, row) in rows.iter().enumerate() {
                let mut line = div()
                    .flex()
                    .when(r > 0, |d| d.border_t_1().border_color(theme.border))
                    .when(*header && r == 0, |d| d.bg(theme.input));
                for c in 0..columns {
                    let runs = row
                        .cells
                        .get(c)
                        .map(|cell| cell.runs.as_slice())
                        .unwrap_or(&[]);
                    line = line.child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .px(px(10.))
                            .py(px(6.))
                            .when(c > 0, |d| d.border_l_1().border_color(theme.border))
                            .child(rich_text(
                                &format!("{}/{r}/{c}", ctx.path),
                                runs,
                                ctx.size - 1.,
                                ctx.color,
                                &theme,
                                &this.core,
                            )),
                    );
                }
                table = table.child(line);
            }
            table.into_any_element()
        }
        BlockView::Rule => div().w_full().h(px(1.)).bg(theme.border).into_any_element(),
        BlockView::Image { image } => preview_image(this, image, 250., true, ctx.caption, cx),
        BlockView::Card { card: c } => card(this, ctx, c, window, cx),
    }
}

/// Shows a pointing hand while the pointer is over one of `links` in `layout`'s text.
fn link_cursor(layout: gpui::TextLayout, links: Vec<std::ops::Range<usize>>) -> impl IntoElement {
    gpui::canvas(
        |_, _, _| {},
        move |bounds, _, window, _| {
            let pointer = window.mouse_position();
            if !bounds.contains(&pointer) {
                return;
            }
            let Ok(index) = layout.index_for_position(pointer) else { return };
            if links.iter().any(|r| r.contains(&index)) {
                let hitbox = window.insert_hitbox(bounds, gpui::HitboxBehavior::Normal);
                window.set_cursor_style(gpui::CursorStyle::PointingHand, &hitbox);
            }
        },
    )
    .absolute()
    .size_full()
}

/// Line height of a paragraph: Figtree's own plus the 4 pt line spacing of the macOS text, none for large emoji.
fn line_height(size: f32) -> f32 {
    (size * 1.22).round() + if size > 20. { 0. } else { 4. }
}

/// `#rgb` or `#rrggbb` (`ComposerText.color`).
fn hex_color(value: Option<&str>) -> Option<Hsla> {
    let digits = value?.strip_prefix('#')?;
    let expanded: String = if digits.len() == 3 {
        digits.chars().flat_map(|c| [c, c]).collect()
    } else {
        digits.to_owned()
    };
    if expanded.len() != 6 {
        return None;
    }
    u32::from_str_radix(&expanded, 16)
        .ok()
        .map(|v| rgb(v).into())
}

/// Styled runs as one wrapping paragraph; links open through the core.
pub fn rich_text(
    id: &str,
    runs: &[RunView],
    base: f32,
    color: Hsla,
    theme: &Theme,
    core: &Arc<OxideCore>,
) -> AnyElement {
    let mut text = String::new();
    let mut text_runs = Vec::new();
    let mut links: Vec<(std::ops::Range<usize>, String)> = Vec::new();
    // GPUI sizes a paragraph as a whole: the largest run's size stands for the paragraph's.
    let size = runs
        .iter()
        .filter(|r| r.size > 0)
        .map(|r| r.size as f32)
        .fold(base, f32::max);
    for run in runs {
        if run.text.is_empty() {
            continue;
        }
        let start = text.len();
        text.push_str(&run.text);
        let mut font = gpui::font(if run.code { MONO } else { FONT });
        if run.bold || run.mention {
            font.weight = FontWeight::SEMIBOLD;
        }
        if run.italic {
            font.style = FontStyle::Italic;
        }
        let mut fg = hex_color(run.color.as_deref()).unwrap_or(color);
        let mut underline = run.underline.then_some(UnderlineStyle {
            thickness: px(1.),
            ..Default::default()
        });
        if run.mention {
            fg = theme.accent_text;
        }
        if let Some(link) = &run.link {
            fg = theme.accent;
            underline = Some(UnderlineStyle {
                thickness: px(1.),
                color: Some(theme.accent),
                wavy: false,
            });
            links.push((start..text.len(), link.clone()));
        }
        let background = hex_color(run.highlight.as_deref()).or(run.code.then_some(theme.input));
        text_runs.push(TextRun {
            len: run.text.len(),
            font,
            color: fg,
            background_color: background,
            underline,
            strikethrough: run.strike.then_some(StrikethroughStyle {
                thickness: px(1.),
                ..Default::default()
            }),
        });
    }
    let frame = div()
        .min_w_0()
        .text_size(px(size))
        .line_height(px(line_height(size)))
        .text_color(color);
    if text.is_empty() {
        return frame.h(px(line_height(size))).into_any_element();
    }
    // In a message body: selectable with the mouse, the feed listener follows the drag and opens links.
    if let Some(unit) = crate::feed_select::next_unit() {
        let text_runs = match crate::feed_select::selected(&unit, text.len()) {
            Some(range) => crate::feed_select::highlight(text_runs, range, theme.accent.opacity(0.3)),
            None => text_runs,
        };
        let styled = StyledText::new(SharedString::from(text.clone())).with_runs(text_runs);
        let bounds = std::rc::Rc::new(std::cell::Cell::new(None));
        crate::feed_select::register(unit, text, styled.layout().clone(), bounds.clone(), links.clone());
        let link_ranges: Vec<_> = links.iter().map(|(r, _)| r.clone()).collect();
        let layout = styled.layout().clone();
        return frame
            .cursor(gpui::CursorStyle::IBeam)
            .on_children_prepainted(move |b, _, _| bounds.set(b.first().copied()))
            .child(styled)
            // A pointing hand over links, as the text view shows.
            .when(!link_ranges.is_empty(), |d| {
                d.child(link_cursor(layout, link_ranges))
            })
            .into_any_element();
    }
    let styled = StyledText::new(SharedString::from(text)).with_runs(text_runs);
    if links.is_empty() {
        return frame.child(styled).into_any_element();
    }
    let core = core.clone();
    let ranges = links.iter().map(|(r, _)| r.clone()).collect();
    let urls: Vec<String> = links.into_iter().map(|(_, u)| u).collect();
    frame
        .child(
            InteractiveText::new(SharedString::from(format!("text-{id}")), styled).on_click(
                ranges,
                move |ix, _, _| {
                    if let Some(url) = urls.get(ix) {
                        core.send(Action::OpenUrl { url: url.clone() });
                    }
                },
            ),
        )
        .into_any_element()
}

/// Code as written: monospaced, never wrapped (scrolls sideways), with its language and a copy button.
fn code(
    this: &mut Oxide,
    id: &str,
    language: Option<&str>,
    text: &str,
    _cx: &mut Context<Oxide>,
) -> AnyElement {
    let theme = this.theme;
    let copied = text.to_owned();
    div()
        .min_w_0()
        .w_full()
        .flex()
        .flex_col()
        .rounded(px(8.))
        .bg(theme.input)
        .border_1()
        .border_color(theme.border)
        .child(
            div()
                .h(px(26.))
                .pl(px(10.))
                .pr(px(4.))
                .flex()
                .items_center()
                .child(
                    div()
                        .flex_1()
                        .text_size(px(11.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.muted)
                        .child(language.unwrap_or("Code").to_owned()),
                )
                .child(
                    ghost(
                        SharedString::from(format!("code-copy-{id}")),
                        Some(Icon::Copy),
                        tr("ui.message_blocks.copy"),
                        false,
                        this,
                    )
                    .py(px(2.))
                    .text_size(px(11.))
                    .tooltip(widgets::tooltip(tr("ui.message_blocks.copy_code"), theme))
                    .on_click(move |_, _, cx| {
                        cx.write_to_clipboard(ClipboardItem::new_string(copied.clone()))
                    }),
                ),
        )
        .child(div().h(px(1.)).bg(theme.border))
        .child(
            div()
                .id(SharedString::from(format!("code-{id}")))
                .overflow_x_scroll()
                .p(px(10.))
                .font_family(MONO)
                .text_size(px(12.5))
                .line_height(px(17.))
                .text_color(theme.text)
                .whitespace_nowrap()
                .child(text.to_owned()),
        )
        .into_any_element()
}

// MARK: Pictures

const MAX_W: f32 = 620.;
const MAX_H: f32 = 480.;

/// The declared size, scaled down to fit 620 × 480.
fn declared_size(image: &ImageView) -> Option<(f32, f32)> {
    let (w, h) = (image.width? as f32, image.height? as f32);
    if w <= 0. || h <= 0. {
        return None;
    }
    let max_w = MAX_W.min(crate::feed::content_limit());
    let scale = (max_w / w).min(MAX_H / h).min(1.);
    Some((w * scale, h * scale))
}
/// Only a height, as a card's `"height": "24px"` badge: its width follows the picture.
fn declared_height(image: &ImageView) -> Option<f32> {
    if image.width.is_some() {
        return None;
    }
    image
        .height
        .filter(|h| *h > 0)
        .map(|h| (h as f32).min(MAX_H))
}

/// A message picture in a fixed box whether loaded or not, so rows never resize; a click shows it full size.
pub fn preview_image(
    this: &mut Oxide,
    image: &ImageView,
    height: f32,
    savable: bool,
    caption: Option<&Caption>,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    let theme = this.theme;
    let picture = match image.status {
        ImageStatus::Ready { .. } => this.images.images.get(&image.src).cloned(),
        _ => None,
    };
    let small = match (declared_height(image), declared_size(image)) {
        (Some(h), _) => h <= 160.,
        (None, Some((w, h))) => w <= 160. && h <= 160.,
        _ => false,
    };
    let id = SharedString::from(format!("picture-{}", image.src));
    let mut frame = div()
        .id(id.clone())
        .relative()
        .flex_none()
        .rounded(px(8.))
        .overflow_hidden();
    frame = if let (Some((w, h)), true) = (declared_size(image), small) {
        frame.w(px(w)).h(px(h))
    } else if let Some((w, h)) = declared_size(image) {
        // An own width rather than a share of the bubble's, which has none until its content says so.
        let mut d = frame.w(px(w)).max_w_full();
        d.style().aspect_ratio = Some(w / h);
        d
    } else if let Some(h) = declared_height(image) {
        let width = picture
            .as_ref()
            .map(|p| {
                let size = p.size(0);
                let (pw, ph) = (size.width.0 as f32, size.height.0 as f32);
                if pw > 0. && ph > 0. {
                    (h * pw / ph).min(MAX_W).min(crate::feed::content_limit())
                } else {
                    h
                }
            })
            .unwrap_or(h);
        frame.w(px(width)).h(px(h))
    } else {
        frame.w(px(420f32.min(crate::feed::content_limit()))).max_w_full().h(px(height))
    };
    if picture.is_none() {
        frame = frame.bg(theme.input);
    }
    let content = match (&image.status, picture) {
        (ImageStatus::Ready { .. }, Some(picture)) => {
            let src = image.src.clone();
            let alt = image.alt.clone();
            let caption = caption.cloned();
            // Fitted at the top left of a box without a declared size, as SwiftUI's `scaledToFit`.
            let fitted =
                (declared_size(image).is_none() && declared_height(image).is_none()).then(|| {
                    let native = picture.size(0);
                    let (pw, ph) = (native.width.0 as f32, native.height.0 as f32);
                    let scale = if pw > 0. && ph > 0. {
                        (420f32.min(crate::feed::content_limit()) / pw).min(height / ph)
                    } else {
                        1.
                    };
                    (pw * scale, ph * scale)
                });
            let picture = match fitted {
                Some((w, h)) => img(picture).w(px(w)).h(px(h)).max_w_full().rounded(px(8.)),
                None => img(picture).size_full().object_fit(ObjectFit::Contain),
            };
            return frame
                .cursor_pointer()
                .child(picture)
                .tooltip(widgets::tooltip(tr("ui.message_row.enlarge_image"), theme))
                .on_click(cx.listener(move |this, _, window, cx| {
                    crate::image_viewer::open(
                        this,
                        src.clone(),
                        alt.clone(),
                        caption.clone(),
                        savable,
                        window,
                        cx,
                    );
                }))
                .into_any_element();
        }
        (ImageStatus::Failed { reason }, _) if small => {
            let core = this.core.clone();
            let src = image.src.clone();
            frame = frame
                .cursor_pointer()
                .tooltip(widgets::tooltip(
                    format!("{reason} · {}", tr("ui.message_row.retry_image")),
                    theme,
                ))
                .on_click(move |_, _, _| core.send(Action::RetryImage { src: src.clone() }));
            div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .child(icon(Icon::Image, px(14.), theme.muted))
                .into_any_element()
        }
        (ImageStatus::Failed { reason }, _) => {
            let core = this.core.clone();
            let src = image.src.clone();
            div()
                .p(px(10.))
                .flex()
                .flex_col()
                .items_start()
                .gap(px(6.))
                .child(widgets::muted(reason.clone(), 11., &theme))
                .child(
                    ghost(
                        SharedString::from(format!("retry-{}", image.src)),
                        None,
                        tr("ui.message_row.retry_image"),
                        false,
                        this,
                    )
                    .on_click(move |_, _, _| core.send(Action::RetryImage { src: src.clone() })),
                )
                .into_any_element()
        }
        // Loading, or ready and still being decoded here.
        _ => div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .child(widgets::spinner(
                SharedString::from(format!("picture-spin-{}", image.src)),
                14.,
                theme.muted,
            ))
            .into_any_element(),
    };
    frame.child(content).into_any_element()
}

// MARK: Link previews

/// A link's preview under a message: the page's picture, title, description and site; a click opens the link.
pub fn link_preview(
    this: &mut Oxide,
    preview: &LinkPreviewView,
    message: &str,
    _cx: &mut Context<Oxide>,
) -> AnyElement {
    let theme = this.theme;
    let picture = preview.image.as_ref().map(|image| {
        let ready = match image.status {
            ImageStatus::Ready { .. } => this.images.images.get(&image.src).cloned(),
            _ => None,
        };
        let frame = div().w_full().h(px(150.)).flex_none().overflow_hidden();
        match (ready, &image.status) {
            (Some(picture), _) => {
                frame.child(img(picture).size_full().object_fit(ObjectFit::Cover))
            }
            (None, ImageStatus::Failed { reason }) => {
                let core = this.core.clone();
                let src = image.src.clone();
                frame
                    .bg(theme.input)
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(6.))
                    .p(px(10.))
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(theme.muted)
                            .text_center()
                            .line_clamp(3)
                            .child(reason.clone()),
                    )
                    .child(
                        ghost(
                            SharedString::from(format!("preview-retry-{message}")),
                            None,
                            tr("ui.message_row.retry_image"),
                            false,
                            this,
                        )
                        .on_click(move |_, _, _| {
                            core.send(Action::RetryImage { src: src.clone() })
                        }),
                    )
            }
            _ => frame
                .bg(theme.input)
                .flex()
                .items_center()
                .justify_center()
                .child(widgets::spinner(
                    SharedString::from(format!("preview-spin-{message}")),
                    14.,
                    theme.muted,
                )),
        }
    });
    let core = this.core.clone();
    let url = preview.url.clone();
    widgets::card(&theme)
        .id(SharedString::from(format!("link-preview-{message}")))
        .rounded(px(10.))
        .overflow_hidden()
        .flex()
        .flex_col()
        .cursor_pointer()
        .children(picture)
        .child(
            div()
                .px(px(12.))
                .py(px(10.))
                .flex()
                .flex_col()
                .gap(px(3.))
                .when_some(preview.title.clone(), |d, t| {
                    d.child(
                        div()
                            .text_size(px(13.))
                            .font_weight(FontWeight::SEMIBOLD)
                            .line_clamp(2)
                            .child(t),
                    )
                })
                .when_some(preview.description.clone(), |d, t| {
                    d.child(
                        div()
                            .text_size(px(12.))
                            .text_color(theme.muted)
                            .line_clamp(3)
                            .child(t),
                    )
                })
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .child(icon(Icon::Link, px(10.), theme.muted))
                        .child(
                            div()
                                .truncate()
                                .text_size(px(11.))
                                .text_color(theme.muted)
                                .child(preview.site.clone()),
                        ),
                ),
        )
        .tooltip(widgets::tooltip(preview.url.clone(), theme))
        .on_click(move |_, _, _| core.send(Action::OpenUrl { url: url.clone() }))
        .into_any_element()
}

// MARK: Cards

fn input_key(message: &str, card: u32, input: &str) -> String {
    format!("{message}/{card}/{input}")
}

/// Every input of a card's items, nested ones included.
fn card_inputs(items: &[CardItemView]) -> Vec<&CardInputView> {
    items
        .iter()
        .flat_map(|item| match item {
            CardItemView::Input { input } => vec![input],
            CardItemView::Columns { columns } => {
                columns.iter().flat_map(|c| card_inputs(&c.items)).collect()
            }
            CardItemView::Emphasis { items } => card_inputs(items),
            _ => vec![],
        })
        .collect()
}

/// The value typed into an input, or its initial one.
fn input_value(this: &Oxide, key: &str, input: &CardInputView, cx: &gpui::App) -> String {
    if let Some(entity) = this.ui.conversation.card_inputs.get(key) {
        return entity.read(cx).text().to_owned();
    }
    this.ui
        .conversation
        .card_values
        .get(key)
        .cloned()
        .unwrap_or_else(|| input.value.clone())
}

/// A bot or connector card; typed input values stay here until a button sends them to the bot.
fn card(
    this: &mut Oxide,
    ctx: &Ctx,
    card: &CardView,
    window: &mut Window,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    // A card's texts are not part of the message's selection (Swift's `inFeedSelection: false`).
    let saved = crate::feed_select::suspend();
    let drawn = card_inner(this, ctx, card, window, cx);
    crate::feed_select::resume(saved);
    drawn
}

fn card_inner(
    this: &mut Oxide,
    ctx: &Ctx,
    card: &CardView,
    window: &mut Window,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    let theme = this.theme;
    let message = ctx.message.to_owned();
    let mut column = div()
        .relative()
        .w_full()
        .max_w(px(440.))
        .min_w_0()
        .p(px(12.))
        .when(card.accent.is_some(), |d| d.pt(px(15.)))
        .flex()
        .flex_col()
        .gap(px(8.))
        .rounded(px(10.))
        .overflow_hidden()
        .bg(theme.surface)
        .border_1()
        .border_color(theme.border)
        .text_size(px(13.))
        .when_some(card.accent, |d, accent| {
            // A connector card's `themeColor`, as Teams draws it: a bar along the top edge.
            d.child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .right_0()
                    .h(px(3.))
                    .bg(rgb(accent)),
            )
        });
    if let Some(title) = &card.title {
        column = column.child(
            div()
                .text_size(px(15.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(title.clone()),
        );
    }
    if let Some(subtitle) = &card.subtitle {
        column = column.child(widgets::muted(subtitle.clone(), 13., &theme));
    }
    let inner = ctx.child(format!("card{}", card.index));
    column = column.children(card_items(
        this,
        &inner,
        card.index,
        &card.items,
        window,
        cx,
    ));
    if !card.actions.is_empty() {
        let mut row = if card.actions.len() <= 3 {
            div().flex().flex_wrap().items_center().gap(px(8.))
        } else {
            div().flex().flex_col().items_start().gap(px(6.))
        };
        for (ix, action) in card.actions.iter().enumerate() {
            let disabled = action.kind == CardActionKind::Unsupported || card.busy;
            let help = action.url.clone().unwrap_or_else(|| {
                if action.kind == CardActionKind::Unsupported {
                    tr("ui.message_blocks.only_in_teams")
                } else {
                    action.title.clone()
                }
            });
            let button = outline(
                SharedString::from(format!("{}/action{ix}", inner.path)),
                action.title.clone(),
                this,
            )
            .tooltip(widgets::tooltip(help, theme));
            let button = if disabled {
                widgets::disabled(button, true)
            } else {
                let (url, message, items, index) = (
                    action.url.clone(),
                    message.clone(),
                    card.items.clone(),
                    card.index,
                );
                button.on_click(cx.listener(move |this, _, _, cx| {
                    if let Some(url) = &url {
                        this.send(Action::OpenUrl { url: url.clone() });
                        return;
                    }
                    let inputs = card_inputs(&items)
                        .into_iter()
                        .map(|input| CardInputValue {
                            id: input.id.clone(),
                            value: input_value(
                                this,
                                &input_key(&message, index, &input.id),
                                input,
                                cx,
                            ),
                        })
                        .collect();
                    this.send(Action::CardAction {
                        message_id: message.clone(),
                        card: index,
                        action: ix as u32,
                        inputs,
                    });
                }))
            };
            row = row.child(button);
        }
        if card.busy {
            row = row.child(widgets::spinner(
                SharedString::from(format!("{}/busy", inner.path)),
                14.,
                theme.muted,
            ));
        }
        column = column.child(row.pt(px(2.)));
    }
    if let Some(footer) = &card.footer {
        column = column.child(widgets::muted(footer.clone(), 12., &theme));
    }
    column.into_any_element()
}

/// A card's items, top to bottom; a ColumnSet's columns side by side.
fn card_items(
    this: &mut Oxide,
    ctx: &Ctx,
    card: u32,
    items: &[CardItemView],
    window: &mut Window,
    cx: &mut Context<Oxide>,
) -> Vec<AnyElement> {
    let theme = this.theme;
    let mut out = Vec::new();
    for (ix, item) in items.iter().enumerate() {
        let ctx = ctx.child(ix);
        out.push(match item {
            CardItemView::Text { runs, muted } => {
                let color = if *muted { theme.muted } else { theme.text };
                rich_text(&ctx.path, runs, 14., color, &theme, &this.core)
            }
            CardItemView::Blocks { blocks } => {
                let inner = Ctx {
                    size: 14.,
                    ..ctx.clone()
                };
                block_list(this, &inner, blocks, BLOCK_SPACING, window, cx)
            }
            CardItemView::Gap { points, separator } => {
                if *separator {
                    div()
                        .py(px(*points as f32 / 2.))
                        .child(div().h(px(1.)).bg(theme.border))
                        .into_any_element()
                } else {
                    div().h(px(*points as f32)).into_any_element()
                }
            }
            CardItemView::Facts { facts } => div()
                .flex()
                .flex_col()
                .gap(px(4.))
                .children(facts.iter().map(|fact| {
                    div()
                        .flex()
                        .items_baseline()
                        .gap(px(12.))
                        .child(
                            div()
                                .flex_none()
                                .max_w(px(180.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(fact.title.clone()),
                        )
                        .child(div().flex_1().min_w_0().child(fact.value.clone()))
                }))
                .into_any_element(),
            CardItemView::Image { image } => {
                preview_image(this, image, 180., true, ctx.caption, cx)
            }
            CardItemView::Input { input } => {
                card_input(this, &input_key(ctx.message, card, &input.id), input, cx)
            }
            CardItemView::Columns { columns } => {
                let mut row = div().w_full().flex().items_start().gap(px(8.));
                for (c, column) in columns.iter().enumerate() {
                    let inner = ctx.child(format!("c{c}"));
                    let content = div().flex().flex_col().gap(px(8.)).children(card_items(
                        this,
                        &inner,
                        card,
                        &column.items,
                        window,
                        cx,
                    ));
                    row = row.child(match column.width {
                        // GPUI sizes a text by its narrowest wrap here: an `auto` column keeps its lines whole.
                        CardColumnWidth::Auto => content.flex_none().whitespace_nowrap(),
                        CardColumnWidth::Points { points } => {
                            content.flex_none().w(px(points as f32))
                        }
                        CardColumnWidth::Stretch => content.flex_1().min_w_0(),
                        CardColumnWidth::Weight { weight } => content
                            .flex_grow()
                            .flex_basis(px(0.))
                            .min_w_0()
                            .map(|mut d| {
                                d.style().flex_grow = Some(weight.max(1) as f32);
                                d
                            }),
                    });
                }
                row.into_any_element()
            }
            CardItemView::Emphasis { items } => div()
                .w_full()
                .p(px(10.))
                .flex()
                .flex_col()
                .gap(px(8.))
                .rounded(px(8.))
                .bg(theme.input)
                .border_1()
                .border_color(theme.border)
                .children(card_items(this, &ctx, card, items, window, cx))
                .into_any_element(),
        });
    }
    out
}

/// One `Input.*` of a card, editing its value as the card's JSON expects it.
fn card_input(
    this: &mut Oxide,
    key: &str,
    input: &CardInputView,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    let theme = this.theme;
    let label = input.label.as_ref().map(|l| {
        div()
            .text_size(px(12.))
            .font_weight(FontWeight::SEMIBOLD)
            .child(if input.required {
                format!("{l} *")
            } else {
                l.clone()
            })
    });
    let field = match &input.kind {
        CardInputKind::Text {
            placeholder,
            multiline,
        } => text_input(this, key, input, placeholder, *multiline, cx),
        CardInputKind::Number { placeholder } => {
            text_input(this, key, input, placeholder, false, cx)
        }
        // No date or time picker here: typed as the card's JSON expects them.
        CardInputKind::Date => text_input(
            this,
            key,
            input,
            crate::oxide::text("AAAA-MM-JJ", "YYYY-MM-DD"),
            false,
            cx,
        ),
        CardInputKind::Time => text_input(this, key, input, "HH:MM", false, cx),
        CardInputKind::Toggle { title, on, off } => {
            let value = this
                .ui
                .conversation
                .card_values
                .get(key)
                .cloned()
                .unwrap_or_else(|| {
                    if input.value.is_empty() {
                        off.clone()
                    } else {
                        input.value.clone()
                    }
                });
            let checked = value == *on;
            let (key, on, off) = (key.to_owned(), on.clone(), off.clone());
            widgets::checkbox(
                SharedString::from(format!("card-toggle-{key}")),
                checked,
                title.clone(),
                &theme,
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                let next = if checked { off.clone() } else { on.clone() };
                this.ui.conversation.card_values.insert(key.clone(), next);
                cx.notify();
            }))
            .into_any_element()
        }
        CardInputKind::Choice {
            choices,
            multi: true,
        } => {
            let value = this
                .ui
                .conversation
                .card_values
                .get(key)
                .cloned()
                .unwrap_or_else(|| input.value.clone());
            let selected: Vec<String> = value
                .split(',')
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect();
            let mut column = div().flex().flex_col().gap_1();
            for choice in choices {
                let on = selected.contains(&choice.value);
                let (key, value, all) = (key.to_owned(), choice.value.clone(), choices.clone());
                let selected = selected.clone();
                column = column.child(
                    widgets::checkbox(
                        SharedString::from(format!("card-choice-{key}-{}", choice.value)),
                        on,
                        choice.title.clone(),
                        &theme,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let mut set = selected.clone();
                        if on {
                            set.retain(|v| *v != value);
                        } else {
                            set.push(value.clone());
                        }
                        let joined = all
                            .iter()
                            .map(|c| c.value.clone())
                            .filter(|v| set.contains(v))
                            .collect::<Vec<_>>()
                            .join(",");
                        this.ui.conversation.card_values.insert(key.clone(), joined);
                        cx.notify();
                    })),
                );
            }
            column.into_any_element()
        }
        CardInputKind::Choice {
            choices,
            multi: false,
        } => {
            let value = this
                .ui
                .conversation
                .card_values
                .get(key)
                .cloned()
                .unwrap_or_else(|| input.value.clone());
            let title = choices
                .iter()
                .find(|c| c.value == value)
                .map(|c| c.title.clone())
                .unwrap_or_else(|| tr("ui.message_blocks.select"));
            let popup = format!("card-choice:{key}");
            let open = this.popup_open(&popup);
            let mut items = vec![(String::new(), tr("ui.message_blocks.select"))];
            items.extend(choices.iter().map(|c| (c.value.clone(), c.title.clone())));
            let menu: Vec<AnyElement> = items
                .into_iter()
                .enumerate()
                .map(|(ix, (v, t))| {
                    let key = key.to_owned();
                    widgets::menu_item(
                        SharedString::from(format!("card-choice-item-{ix}")),
                        (v == value).then_some(Icon::Check),
                        t,
                        &theme,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.ui
                            .conversation
                            .card_values
                            .insert(key.clone(), v.clone());
                        this.close_popup(cx);
                    }))
                    .into_any_element()
                })
                .collect();
            let close = cx.listener(|this, _: &(), _, cx| this.close_popup(cx));
            div()
                .relative()
                .child(
                    outline(
                        SharedString::from(format!("card-select-{key}")),
                        title,
                        this,
                    )
                    .child(icon(Icon::ChevronDown, px(12.), theme.muted))
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.toggle_popup(popup.clone(), cx)),
                    ),
                )
                .when(open, |d| {
                    d.child(widgets::popover(
                        Corner::TopLeft,
                        div().flex().flex_col().children(menu),
                        &theme,
                        move |w, cx| close(&(), w, cx),
                    ))
                })
                .into_any_element()
        }
    };
    div()
        .flex()
        .flex_col()
        .gap(px(4.))
        .children(label)
        .child(field)
        .into_any_element()
}

/// A typed input: its editor lives in the conversation's state until the message leaves the feed.
fn text_input(
    this: &mut Oxide,
    key: &str,
    input: &CardInputView,
    placeholder: &str,
    multiline: bool,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    let theme = this.theme;
    let entity = match this.ui.conversation.card_inputs.get(key) {
        Some(entity) => entity.clone(),
        None => {
            let (placeholder, value) = (placeholder.to_owned(), input.value.clone());
            let entity = cx.new(|cx| {
                let mut field = crate::input::TextInput::new(cx, placeholder);
                if multiline {
                    field = field.multiline(false);
                }
                field.set_text(value, None, cx);
                field
            });
            this.ui
                .conversation
                .card_inputs
                .insert(key.to_owned(), entity.clone());
            entity
        }
    };
    widgets::text_field(&entity, &theme, cx)
        .w_full()
        .when(multiline, |d| d.min_h(px(60.)))
        .into_any_element()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(t: &str) -> BlockView {
        BlockView::Text {
            runs: vec![RunView {
                text: t.into(),
                link: None,
                bold: false,
                italic: false,
                code: false,
                underline: false,
                strike: false,
                color: None,
                highlight: None,
                size: 0,
                mention: false,
            }],
        }
    }

    #[test]
    fn emoji_alone_are_large() {
        assert!(emoji_only(&[text("🙂")]));
        assert!(emoji_only(&[text("👍 🎉 ❤️")]));
        assert!(emoji_only(&[text("👍🏽")]));
        assert!(!emoji_only(&[text("👍👍👍👍")]));
        assert!(!emoji_only(&[text("ok 👍")]));
        assert!(!emoji_only(&[text("123")]));
        assert!(!emoji_only(&[text("🙂"), text("🙂")]));
    }

    #[test]
    fn colors_parse_short_and_long() {
        assert!(hex_color(Some("#fff")).is_some());
        assert!(hex_color(Some("#ff0000")).is_some());
        assert!(hex_color(Some("red")).is_none());
        assert!(hex_color(None).is_none());
    }

    #[test]
    fn excerpt_flattens_and_cuts() {
        assert_eq!(excerpt(&[text("a\nb"), text("c")], 10), "a b c");
        assert_eq!(excerpt(&[text("abcdef")], 3), "abc…");
    }

    #[test]
    fn declared_sizes_fit_the_web_client_limits() {
        let image = |w: Option<u32>, h: Option<u32>| ImageView {
            src: "s".into(),
            alt: String::new(),
            status: ImageStatus::Loading,
            width: w,
            height: h,
        };
        assert_eq!(
            declared_size(&image(Some(1240), Some(480))),
            Some((620., 240.))
        );
        assert_eq!(
            declared_size(&image(Some(100), Some(50))),
            Some((100., 50.))
        );
        assert_eq!(declared_size(&image(None, Some(24))), None);
        assert_eq!(declared_height(&image(None, Some(24))), Some(24.));
    }
}

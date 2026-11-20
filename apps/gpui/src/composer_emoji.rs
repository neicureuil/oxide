//! The emoji picker (`EmojiPicker.swift`): Teams' catalog in the app's language, its categories, a search over
//! names and keywords, recent picks; Teams artwork with the native glyph while it loads.
use crate::composer;
use crate::composer_mention::fold;
use crate::composer_thumbs::{Thumb, thumb};
use crate::input::{InputEvent, TextInput};
use crate::oxide::{Oxide, tr, trf};
use crate::widgets;
use gpui::{
    AnyElement, Context, Entity, FontWeight, ObjectFit, Subscription, Window, div, img, prelude::*,
    px, uniform_list,
};
use std::ops::Range;
use std::rc::Rc;
use teams_core::ComposerEmoji;

const COLUMNS: usize = 8;
const CELL: f32 = 36.;
const GAP: f32 = 3.;
const RECENT_MAX: usize = 24;
/// The glyph each category tab shows, from the catalog when it has it.
const ICONS: [&str; 9] = ["🙂", "👋", "👤", "🐵", "🍇", "🌍", "👓", "🎃", "🔣"];

pub struct Picker {
    search: Entity<TextInput>,
    category: usize,
    /// Ids of the last picks, most recent first (Swift's `composerRecentEmoji`).
    recent: Vec<String>,
    /// Every emoji's folded search text by category and position, built when the picker opens.
    index: Vec<(usize, usize, String)>,
    _subscription: Subscription,
}

impl Picker {
    pub fn new(window: &mut Window, cx: &mut Context<Oxide>) -> Self {
        let search = cx.new(|cx| TextInput::new(cx, tr("ui.emoji_picker.search_emoji")));
        let subscription = cx.subscribe_in(
            &search,
            window,
            |this: &mut Oxide, _, event: &InputEvent, window, cx| match event {
                InputEvent::Cancel => {
                    this.close_popup(cx);
                    this.ui.composer.input.read(cx).focus(window);
                }
                InputEvent::Submit => {
                    if let Some(first) = results(this, cx).first().copied() {
                        choose(this, first, window, cx);
                    }
                }
                _ => cx.notify(),
            },
        );
        Self {
            search,
            category: 0,
            recent: load_recent(),
            index: Vec::new(),
            _subscription: subscription,
        }
    }
}

fn recent_path() -> Option<std::path::PathBuf> {
    let home = std::env::var_os("HOME")?;
    Some(
        std::path::PathBuf::from(home)
            .join("Library/Application Support/oxide-gpui/composer-recent-emoji.txt"),
    )
}
fn load_recent() -> Vec<String> {
    recent_path()
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|s| {
            s.trim()
                .split('|')
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default()
}
fn save_recent(ids: &[String]) {
    let Some(path) = recent_path() else { return };
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let _ = std::fs::write(path, ids.join("|"));
}

pub fn opened(this: &mut Oxide, cx: &mut Context<Oxide>) {
    let picker = &mut this.ui.composer.emoji;
    picker.index = this
        .emoji
        .iter()
        .enumerate()
        .flat_map(|(c, category)| {
            category.items.iter().enumerate().map(move |(i, e)| {
                (
                    c,
                    i,
                    fold(&format!("{} {} {} {}", e.name, e.keywords, e.id, e.text)),
                )
            })
        })
        .collect();
    picker.search.update(cx, |s, cx| s.set_text("", None, cx));
}

/// The picker closed: its index goes.
pub fn closed(this: &mut Oxide) {
    let picker = &mut this.ui.composer.emoji;
    if !picker.index.is_empty() {
        picker.index = Vec::new();
    }
}

pub fn search(this: &mut Oxide, query: &str, window: &mut Window, cx: &mut Context<Oxide>) {
    let search = this.ui.composer.emoji.search.clone();
    search.update(cx, |s, cx| {
        s.set_text(query, None, cx);
        s.focus(window);
    });
}

/// The emoji shown: the chosen category's, or those matching the search.
fn results(this: &Oxide, cx: &Context<Oxide>) -> Vec<(usize, usize)> {
    let picker = &this.ui.composer.emoji;
    let query = picker.search.read(cx).text().trim().to_owned();
    if query.is_empty() {
        let c = picker.category;
        return this
            .emoji
            .get(c)
            .map(|cat| (0..cat.items.len()).map(|i| (c, i)).collect())
            .unwrap_or_default();
    }
    let folded = fold(&query);
    picker
        .index
        .iter()
        .filter(|(_, _, key)| key.contains(&folded))
        .map(|(c, i, _)| (*c, *i))
        .collect()
}

fn item(this: &Oxide, (c, i): (usize, usize)) -> Option<&ComposerEmoji> {
    this.emoji.get(c)?.items.get(i)
}

fn choose(this: &mut Oxide, at: (usize, usize), window: &mut Window, cx: &mut Context<Oxide>) {
    let Some(emoji) = item(this, at).cloned() else {
        return;
    };
    let recent = &mut this.ui.composer.emoji.recent;
    recent.retain(|id| *id != emoji.id);
    recent.insert(0, emoji.id.clone());
    recent.truncate(RECENT_MAX);
    save_recent(recent);
    this.close_popup(cx);
    composer::replace(this, &emoji.text, None, None, Some(emoji.id.clone()), cx);
    this.ui.composer.input.read(cx).focus(window);
}

/// Teams artwork for an emoji, by its id.
pub fn artwork_url(id: &str) -> String {
    format!(
        "https://teams.public.onecdn.static.microsoft/evergreen-assets/personal-expressions/v2/assets/emoticons/{id}/default/40_f.png"
    )
}

/// The artwork at `size`, or the glyph while it loads or when it is missing.
fn glyph(
    this: &mut Oxide,
    emoji: &ComposerEmoji,
    size: f32,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    let url = artwork_url(&emoji.id);
    match thumb(this, &url, cx) {
        Thumb::Ready(picture) => {
            let artwork = img(picture).size(px(size)).object_fit(ObjectFit::Contain);
            // Just arrived with its batch: the artwork fades in over the glyph fading out, both on the
            // same curve, rather than replacing it at a stroke.
            if this.ui.composer.thumbs.fresh(&url) {
                let glyph = div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(size * 24. / 28.))
                    .child(emoji.text.clone());
                div()
                    .relative()
                    .size(px(size))
                    .child(crate::motion::animate(
                        glyph,
                        gpui::SharedString::from(format!("emoji-out-{}", emoji.id)),
                        crate::motion::Curve::Standard,
                        |d, t| d.opacity(1. - t),
                    ))
                    .child(crate::motion::fade_in(
                        div().absolute().inset_0().child(artwork),
                        gpui::SharedString::from(format!("emoji-in-{}", emoji.id)),
                    ))
                    .into_any_element()
            } else {
                artwork.into_any_element()
            }
        }
        _ => div()
            .size(px(size))
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(size * 24. / 28.))
            .child(emoji.text.clone())
            .into_any_element(),
    }
}

#[derive(Clone)]
enum Row {
    Header(String),
    Note(String),
    Items(Vec<(usize, usize)>),
}

pub fn picker(this: &mut Oxide, _: &mut Window, cx: &mut Context<Oxide>) -> AnyElement {
    let theme = this.theme;
    let picker = &this.ui.composer.emoji;
    let query = picker.search.read(cx).text().trim().to_owned();
    let category = picker.category;
    let search = picker.search.clone();
    let results = results(this, cx);
    let mut rows = Vec::new();
    if query.is_empty() {
        let recent: Vec<(usize, usize)> = this
            .ui
            .composer
            .emoji
            .recent
            .iter()
            .filter_map(|id| locate(this, id))
            .collect();
        if !recent.is_empty() {
            rows.push(Row::Header(tr("ui.emoji_picker.recent")));
            rows.extend(recent.chunks(COLUMNS).map(|c| Row::Items(c.to_vec())));
        }
    }
    rows.push(Row::Header(if !query.is_empty() {
        trf(
            "ui.emoji_picker.results",
            &[("resultsCount", &results.len().to_string())],
        )
    } else {
        this.emoji
            .get(category)
            .map(|c| c.name.clone())
            .unwrap_or_else(|| tr("ui.emoji_picker.emoji"))
    }));
    if this.emoji.is_empty() {
        rows.push(Row::Note(tr("ui.emoji_picker.downloading")));
    } else if results.is_empty() {
        rows.push(Row::Note(tr("ui.emoji_picker.no_emoji_found")));
    }
    rows.extend(results.chunks(COLUMNS).map(|c| Row::Items(c.to_vec())));
    let rows = Rc::new(rows);

    let mut tabs = div().flex().gap(px(1.));
    for ix in 0..this.emoji.len() {
        let name = this.emoji[ix].name.clone();
        let shown = ICONS
            .get(ix)
            .and_then(|g| {
                this.emoji
                    .iter()
                    .flat_map(|c| &c.items)
                    .find(|e| e.text.replace('\u{FE0F}', "") == *g)
                    .cloned()
            })
            .or_else(|| this.emoji[ix].items.first().cloned());
        let face = match shown {
            Some(e) => glyph(this, &e, 22., cx),
            None => div().child("•").into_any_element(),
        };
        let hover = theme.input;
        tabs = tabs.child(
            div()
                .id(("emoji-tab", ix))
                .w(px(34.))
                .h(px(32.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.))
                .cursor_pointer()
                .when(ix == category, |d| d.bg(theme.accent.opacity(0.15)))
                .when(ix != category, move |d| d.hover(move |s| s.bg(hover)))
                .tooltip(widgets::tooltip(name, theme))
                .child(face)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.ui.composer.emoji.category = ix;
                    let search = this.ui.composer.emoji.search.clone();
                    search.update(cx, |s, cx| s.set_text("", None, cx));
                    cx.notify();
                })),
        );
    }

    let count = rows.len();
    let list = uniform_list(
        "emoji-grid",
        count,
        cx.processor(move |this: &mut Oxide, range: Range<usize>, _window, cx| {
            range
                .filter_map(|ix| rows.get(ix).cloned().map(|row| (ix, row)))
                .map(|(ix, row)| render_row(this, ix, row, cx))
                .collect::<Vec<_>>()
        }),
    )
    .h(px(260.))
    .w_full();

    div()
        .w(px(340.))
        .p(px(12.))
        .flex()
        .flex_col()
        .gap(px(10.))
        .child(
            div()
                .text_size(px(15.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(tr("ui.emoji_picker.emoji")),
        )
        .child(widgets::text_field(&search, &theme, cx))
        .child(tabs)
        .child(list)
        .into_any_element()
}

fn locate(this: &Oxide, id: &str) -> Option<(usize, usize)> {
    this.emoji
        .iter()
        .enumerate()
        .find_map(|(c, cat)| cat.items.iter().position(|e| e.id == id).map(|i| (c, i)))
}

fn render_row(this: &mut Oxide, ix: usize, row: Row, cx: &mut Context<Oxide>) -> AnyElement {
    let theme = this.theme;
    let frame = div().h(px(CELL + GAP)).w_full().flex().items_center();
    match row {
        Row::Header(text) => frame
            .items_end()
            .pb(px(4.))
            .text_size(px(12.))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(theme.muted)
            .child(text)
            .into_any_element(),
        Row::Note(text) => frame
            .text_size(px(13.))
            .text_color(theme.muted)
            .child(text)
            .into_any_element(),
        Row::Items(items) => {
            let mut line = frame.gap(px(GAP));
            for (n, at) in items.into_iter().enumerate() {
                let Some(emoji) = item(this, at).cloned() else {
                    continue;
                };
                let face = glyph(this, &emoji, 28., cx);
                let hover = theme.input;
                line = line.child(
                    div()
                        .id(("emoji", ix * COLUMNS + n))
                        .size(px(CELL))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded(px(6.))
                        .cursor_pointer()
                        .hover(move |s| s.bg(hover))
                        .tooltip(widgets::tooltip(emoji.name.clone(), theme))
                        .child(face)
                        .on_click(
                            cx.listener(move |this, _, window, cx| choose(this, at, window, cx)),
                        ),
                );
            }
            line.into_any_element()
        }
    }
}

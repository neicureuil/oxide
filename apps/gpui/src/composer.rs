//! The compose box (`Composer.swift`, `RichDraft.swift`, `DraftInput.swift`): the rich editor bound to the core's
//! draft, the channel subject, delivery options, attachments, link preview and extension cards, and the tools under
//! it. The same editor edits a message in place (`MessageRow.swift`'s `MessageEditor`), see [`message_editor`].
use crate::composer_mention::{self, Mentions};
use crate::icons::{Icon, icon};
use crate::input::{IMAGE_CHARACTER, InputEvent, RichEvent, TextInput, field};
use crate::oxide::{Oxide, tr};
use crate::theme::Theme;
use crate::widgets::{self, ButtonKind};
use crate::{composer_emoji, composer_extensions, composer_fun, composer_thumbs, composer_toolbar};
use gpui::{
    AnyElement, Context, Corner, Entity, ExternalPaths, FontWeight, MouseDownEvent,
    PathPromptOptions, ScrollHandle, Subscription, Window, anchored, deferred, div, img,
    prelude::*, px,
};
use std::ops::Range;
use std::time::{Duration, Instant};
use teams_core::{Action, ComposerView, FileAction, Importance, LinkPreviewAction};

/// Keys of the composer's popovers in `Oxide::popup`.
pub const EMOJI: &str = "composer:emoji";
pub const FUN: &str = "composer:fun";
pub const APPS: &str = "composer:apps";
pub const IMPORTANCE: &str = "composer:importance";

pub struct Ui {
    /// The one editor: the compose box, or the message being edited (never both on screen).
    pub input: Entity<TextInput>,
    subject: Entity<TextInput>,
    /// Subjects sent to the core whose echo is still due (`SyncedTextField`).
    subject_sent: Vec<String>,
    /// The `draft_rev` the editor shows: only a new one replaces its text.
    revision: Option<u64>,
    pub formatting: bool,
    pub mentions: Mentions,
    /// Asked for the focus before the editor was drawn (a thread opening, an edit starting, a debug step).
    focus_wanted: bool,
    /// The core changed the draft: the editor takes the focus back if nothing else has it.
    refocus: bool,
    /// The message the editor last edited, to focus it when an edit starts.
    editing: Option<String>,
    /// A popover closed by the click that lands on its own button: that click must not open it again.
    closed: Option<(String, Instant)>,
    scroll: ScrollHandle,
    /// Length of the text last drawn, to follow the caret at its end.
    drawn_len: usize,
    /// Typing restarts the pause before the spell check.
    spell_pending: u64,
    pub emoji: composer_emoji::Picker,
    pub fun: composer_fun::Picker,
    pub apps: composer_extensions::Flyout,
    pub toolbar: composer_toolbar::Toolbar,
    pub thumbs: composer_thumbs::Thumbs,
    _subscriptions: Vec<Subscription>,
}

impl Ui {
    pub fn new(window: &mut Window, cx: &mut Context<Oxide>) -> Self {
        let input = cx.new(|cx| {
            TextInput::new(cx, tr("ui.composer.type_a_message"))
                .multiline(true)
                .rich()
        });
        let subject = cx.new(|cx| TextInput::new(cx, tr("ui.composer.subject_optional")));
        let subscriptions = vec![
            cx.subscribe_in(
                &input,
                window,
                |this: &mut Oxide, _, event: &InputEvent, window, cx| {
                    on_input(this, event, window, cx)
                },
            ),
            cx.subscribe_in(
                &input,
                window,
                |this: &mut Oxide, _, event: &RichEvent, window, cx| {
                    on_rich(this, event, window, cx)
                },
            ),
            cx.subscribe(&subject, |this: &mut Oxide, _, event: &InputEvent, cx| {
                if *event == InputEvent::Changed {
                    let value = this.ui.composer.subject.read(cx).text().to_owned();
                    this.ui.composer.subject_sent.push(value.clone());
                    this.send(Action::SubjectChanged { value });
                }
            }),
        ];
        Self {
            input,
            subject,
            subject_sent: Vec::new(),
            revision: None,
            formatting: false,
            mentions: Mentions::default(),
            focus_wanted: false,
            refocus: false,
            editing: None,
            closed: None,
            scroll: ScrollHandle::new(),
            drawn_len: 0,
            spell_pending: 0,
            emoji: composer_emoji::Picker::new(window, cx),
            fun: composer_fun::Picker::new(window, cx),
            apps: composer_extensions::Flyout::new(window, cx),
            toolbar: composer_toolbar::Toolbar::new(window, cx),
            thumbs: composer_thumbs::Thumbs::default(),
            _subscriptions: subscriptions,
        }
    }
}

/// Before the first core update.
pub fn empty_view() -> ComposerView {
    ComposerView {
        sending: false,
        draft: String::new(),
        draft_rev: 0,
        draft_cursor: 0,
        mention_ranges: Vec::new(),
        draft_formats: Vec::new(),
        mention_loading: false,
        mention_error: None,
        subject: String::new(),
        importance: Importance::Standard,
        attachments: Vec::new(),
        cards: Vec::new(),
        link_preview: None,
    }
}

/// Asks for the editor's focus, now or once it is drawn (Swift's `Store.focusComposer`): for "Reply" and quotes.
pub fn focus(this: &mut Oxide, cx: &mut Context<Oxide>) {
    this.ui.composer.focus_wanted = true;
    cx.notify();
}

fn on_input(this: &mut Oxide, event: &InputEvent, window: &mut Window, cx: &mut Context<Oxide>) {
    if *event == InputEvent::Changed {
        let input = this.ui.composer.input.read(cx);
        let (value, formats) = (input.text().to_owned(), input.formats());
        this.send(Action::RichDraftChanged { value, formats });
        capture(this, false, cx);
        check_spelling(this, cx);
        return;
    }
    if composer_mention::key(this, event, cx) {
        return;
    }
    let editing = this.shell.as_ref().is_some_and(|s| s.editing.is_some());
    match event {
        InputEvent::Submit => submit(this),
        InputEvent::Up => {
            let input = this.ui.composer.input.read(cx);
            if input.is_empty() && !input.has_marked_text() && !editing {
                edit_last(this);
            }
        }
        InputEvent::Cancel if editing => this.send(Action::CancelEdit),
        _ => {}
    }
    let _ = window;
}

fn on_rich(this: &mut Oxide, event: &RichEvent, window: &mut Window, cx: &mut Context<Oxide>) {
    match event {
        RichEvent::Link => composer_toolbar::request_link(this, window, cx),
        RichEvent::PasteImage(image) => paste_image(this, image, window, cx),
    }
}

fn is_spelling(style: &gpui::HighlightStyle) -> bool {
    style.underline.is_some_and(|u| u.wavy)
}

/// Underlines the misspelled words once typing pauses.
fn check_spelling(this: &mut Oxide, cx: &mut Context<Oxide>) {
    this.ui.composer.spell_pending += 1;
    let pending = this.ui.composer.spell_pending;
    cx.spawn(async move |this, cx| {
        cx.background_executor()
            .timer(Duration::from_millis(500))
            .await;
        let _ = this.update(cx, |this, cx| {
            if this.ui.composer.spell_pending != pending {
                return;
            }
            let red: gpui::Hsla = gpui::rgb(0xE0303A).into();
            this.ui.composer.input.update(cx, |input, cx| {
                let found = crate::composer_spelling::misspelled(input.text(), input.cursor());
                input.highlights.retain(|(_, style)| !is_spelling(style));
                input.highlights.extend(found.into_iter().map(|r| {
                    let style = gpui::HighlightStyle {
                        underline: Some(gpui::UnderlineStyle {
                            thickness: px(1.),
                            color: Some(red),
                            wavy: true,
                        }),
                        ..Default::default()
                    };
                    (r, style)
                }));
                cx.notify();
            });
        });
    })
    .detach();
}

/// Return (⌘ Return with the formatting tools open): sends the draft, or saves the message being edited.
fn submit(this: &mut Oxide) {
    if this.shell.as_ref().is_some_and(|s| s.editing.is_some()) {
        this.send(Action::SaveEdit);
    } else {
        this.send(Action::Send);
    }
}

/// ↑ in an empty composer edits my latest message of the feed it writes to, if it can still be edited.
fn edit_last(this: &mut Oxide) -> bool {
    let reply_open = this.shell.as_ref().is_some_and(|s| s.reply_open);
    let channel = this.conversation.as_ref().is_some_and(|c| c.is_channel);
    let last = |rows: &crate::feed::FeedList<teams_core::MessageView>| {
        (0..rows.len())
            .rev()
            .filter_map(|ix| rows.get(ix))
            .find(|m| m.mine && !m.deleted)
            .map(|m| m.editable.then(|| m.id.clone()))
    };
    let found = if reply_open {
        last(&this.replies)
    } else if channel {
        (0..this.posts.len())
            .rev()
            .filter_map(|ix| this.posts.get(ix))
            .find(|p| p.root.mine && !p.root.deleted)
            .map(|p| p.root.editable.then(|| p.root.id.clone()))
    } else {
        last(&this.messages)
    };
    match found.flatten() {
        Some(id) => {
            this.send(Action::Edit { id });
            true
        }
        None => false,
    }
}

/// The text or caret moved: the mention being typed, if any, and the editor's keys for its list.
fn capture(this: &mut Oxide, core_change: bool, cx: &mut Context<Oxide>) {
    let ranges = this
        .composer
        .as_ref()
        .map(|c| c.mention_ranges.clone())
        .unwrap_or_default();
    let input = this.ui.composer.input.read(cx);
    let value = input.text().to_owned();
    let selection = input.selection();
    let mentions: Vec<Range<usize>> = ranges
        .iter()
        .map(|m| input.utf8(m.start)..input.utf8(m.start + m.length))
        .collect();
    let changed = this
        .ui
        .composer
        .mentions
        .capture(&value, selection, &mentions, core_change);
    if changed && this.ui.composer.mentions.current.is_some() {
        this.send(Action::MentionMembers);
    }
    sync_completing(this, cx);
}

pub fn sync_completing(this: &mut Oxide, cx: &mut Context<Oxide>) {
    let completing = this.ui.composer.mentions.current.is_some();
    this.ui
        .composer
        .input
        .update(cx, |i, _| i.set_completing(completing));
}

/// Replaces the selection (or `range`, in bytes) in the core's draft: an emoji, a mention, a quote.
pub fn replace(
    this: &mut Oxide,
    text: &str,
    range: Option<Range<usize>>,
    mention: Option<String>,
    emoji: Option<String>,
    cx: &mut Context<Oxide>,
) {
    let input = this.ui.composer.input.read(cx);
    let range = range.unwrap_or_else(|| input.selection());
    let (start, end) = (input.utf16(range.start), input.utf16(range.end));
    let style = input.current_format();
    this.send(Action::ReplaceDraft {
        start,
        length: end - start,
        text: text.to_owned(),
        mention,
        emoji,
        style: Some(style),
    });
}

/// An image pasted on its own: uploaded inline under a new id, its placeholder character put in the text.
fn paste_image(
    this: &mut Oxide,
    image: &gpui::Image,
    window: &mut Window,
    cx: &mut Context<Oxide>,
) {
    let png = if image.format() == gpui::ImageFormat::Png {
        image.bytes().to_vec()
    } else {
        let Ok(decoded) = image::load_from_memory(image.bytes()) else {
            return;
        };
        let mut out = std::io::Cursor::new(Vec::new());
        if decoded.write_to(&mut out, image::ImageFormat::Png).is_err() {
            return;
        }
        out.into_inner()
    };
    let Ok((w, h)) = image::ImageReader::new(std::io::Cursor::new(&png))
        .with_guessed_format()
        .map_err(|_| ())
        .and_then(|r| r.into_dimensions().map_err(|_| ()))
    else {
        return;
    };
    // Sizes in points, so a Retina screenshot shows at the size it had on screen.
    let scale = window.scale_factor().max(1.);
    let id = paste_id();
    this.send(Action::Files {
        action: FileAction::PasteImage {
            id: id.clone(),
            png,
            width: ((w as f32 / scale).round() as u32).max(1),
            height: ((h as f32 / scale).round() as u32).max(1),
        },
    });
    this.ui.composer.input.update(cx, |input, cx| {
        let mut style = input.current_format();
        style.image = Some(id);
        style.link = None;
        input.insert_styled(&IMAGE_CHARACTER.to_string(), style, window, cx);
    });
}

fn paste_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNT: AtomicU64 = AtomicU64::new(0);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    format!("{nanos:x}-{:x}", COUNT.fetch_add(1, Ordering::Relaxed))
}

/// The core sent a new `ComposerView`: the editor takes the draft only when the core itself changed it (sent, emoji,
/// mention, another conversation), so typing is never overwritten by its own echo.
pub fn composer_changed(this: &mut Oxide, cx: &mut Context<Oxide>) {
    let Some(view) = this.composer.clone() else {
        return;
    };
    let ui = &mut this.ui.composer;
    if ui.revision != Some(view.draft_rev) {
        let first = ui.revision.is_none();
        ui.revision = Some(view.draft_rev);
        ui.input.update(cx, |i, cx| {
            i.set_rich_text(
                view.draft.clone(),
                &view.draft_formats,
                view.draft_cursor,
                cx,
            )
        });
        ui.refocus = !first;
        capture(this, true, cx);
    }
    // Mentions in the accent colour, once the editor shows the core's text.
    let accent = this.theme.accent;
    this.ui.composer.input.update(cx, |input, cx| {
        if input.text() == view.draft {
            let mentions: Vec<_> = view
                .mention_ranges
                .iter()
                .map(|m| {
                    let style = gpui::HighlightStyle {
                        color: Some(accent),
                        ..Default::default()
                    };
                    (input.utf8(m.start)..input.utf8(m.start + m.length), style)
                })
                .collect();
            input.highlights.retain(|(_, style)| is_spelling(style));
            input.highlights.extend(mentions);
            cx.notify();
        }
    });
    // The subject: the core's echo of what was typed is ignored, only its own changes replace the text.
    let ui = &mut this.ui.composer;
    if let Some(ix) = ui.subject_sent.iter().position(|s| *s == view.subject) {
        ui.subject_sent.drain(..=ix);
    } else if ui.subject.read(cx).text() != view.subject {
        ui.subject_sent.clear();
        ui.subject
            .update(cx, |s, cx| s.set_text(view.subject.clone(), None, cx));
    }
    composer_extensions::changed(this, cx);
}

/// Opens or closes a popover from its button, unless the same click just closed it from outside.
pub fn toggle(this: &mut Oxide, key: &str, cx: &mut Context<Oxide>) {
    if let Some((closed, at)) = this.ui.composer.closed.take()
        && closed == key
        && at.elapsed() < Duration::from_millis(400)
    {
        return;
    }
    let opening = !this.popup_open(key);
    this.toggle_popup(key.to_owned(), cx);
    if opening {
        opened(this, key, cx);
    }
}

/// What a popover does as it opens.
fn opened(this: &mut Oxide, key: &str, cx: &mut Context<Oxide>) {
    match key {
        FUN => composer_fun::opened(this, cx),
        APPS => composer_extensions::opened(this),
        EMOJI => composer_emoji::opened(this, cx),
        _ => {}
    }
}

/// A floating panel above its button, closed by a click outside it.
pub fn float(
    key: &'static str,
    content: impl IntoElement,
    theme: &Theme,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    deferred(
        anchored()
            .anchor(Corner::BottomLeft)
            .snap_to_window_with_margin(px(8.))
            .child(
                div()
                    .occlude()
                    .mb_1()
                    .rounded(px(10.))
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.surface)
                    .shadow_lg()
                    .on_mouse_down_out(cx.listener(move |this, _: &MouseDownEvent, _, cx| {
                        if this.popup_open(key) {
                            this.ui.composer.closed = Some((key.to_owned(), Instant::now()));
                            this.close_popup(cx);
                        }
                    }))
                    .child(content),
            ),
    )
    .with_priority(1)
    .into_any_element()
}

/// Draws the popover `key` above its button when it is open.
pub fn with_popover(
    this: &mut Oxide,
    key: &'static str,
    button: impl IntoElement,
    content: impl FnOnce(&mut Oxide, &mut Window, &mut Context<Oxide>) -> AnyElement,
    window: &mut Window,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    let theme = this.theme;
    let open = this.popup_open(key);
    let panel = open.then(|| {
        let content = content(this, window, cx);
        float(key, content, &theme, cx)
    });
    // Hung from the button's top edge, so the panel opens above it.
    div()
        .relative()
        .flex_none()
        .child(button)
        .children(panel.map(|p| div().absolute().top_0().left_0().child(p)))
        .into_any_element()
}

/// A square ghost button with a text label ("Aa", "GIF", "@").
pub fn text_tool(
    id: &'static str,
    label: &'static str,
    size: f32,
    active: bool,
    theme: &Theme,
) -> gpui::Stateful<gpui::Div> {
    let hover = theme.input;
    div()
        .id(id)
        .size(px(32.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.))
        .cursor_pointer()
        .text_size(px(size))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(if active {
            theme.accent_text
        } else {
            theme.text_soft
        })
        .when(active, |d| d.bg(theme.accent_soft))
        .when(!active, move |d| d.hover(move |s| s.bg(hover)))
        .child(label)
}

/// The editor itself, in its scroll frame no taller than `max`.
fn editor(this: &mut Oxide, max: f32, cx: &mut Context<Oxide>) -> AnyElement {
    let theme = this.theme;
    let ui = &mut this.ui.composer;
    let input = ui.input.clone();
    // Keeps the caret in sight when typing at the end of a long draft.
    let state = input.read(cx);
    let len = state.text().len();
    if len != ui.drawn_len {
        ui.drawn_len = len;
        if state.cursor() == len {
            ui.scroll.scroll_to_bottom();
        }
    }
    div()
        .id("composer-editor")
        .w_full()
        .min_h(px(32.))
        .max_h(px(max))
        .overflow_y_scroll()
        .track_scroll(&ui.scroll)
        .py(px(4.))
        .text_size(px(14.))
        .line_height(px(19.))
        .text_color(theme.text)
        .font_family(crate::oxide::FONT)
        .child(field(&input, widgets::input_colors(&theme), cx))
        .into_any_element()
}

/// The compose box under the feed (`thread`: in the reply pane). Hidden while a message is edited in place.
pub fn composer(
    this: &mut Oxide,
    thread: bool,
    window: &mut Window,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    let theme = this.theme;
    let (read_only, reply_open, editing) = this
        .shell
        .as_ref()
        .map(|s| (s.read_only, s.reply_open, s.editing.clone()))
        .unwrap_or((false, false, None));
    if read_only {
        return div()
            .w_full()
            .p(px(20.))
            .text_size(px(13.))
            .text_color(theme.muted)
            .child(tr("ui.composer.this_conversation_is_read_only"))
            .into_any_element();
    }
    if editing.is_some() {
        return div().into_any_element();
    }
    let channel = this.conversation.as_ref().is_some_and(|c| c.is_channel);
    let placeholder = if reply_open {
        tr("ui.composer.reply_in_thread")
    } else if channel {
        tr("ui.composer.start_a_post")
    } else {
        tr("ui.composer.type_a_message")
    };
    prepare(this, placeholder, window, cx);
    let view = this.composer.clone().unwrap_or_else(empty_view);
    let formatting = this.ui.composer.formatting;

    let mut column = div().flex().flex_col().gap(px(6.));
    if formatting {
        column = column.child(composer_toolbar::toolbar(this, window, cx));
    }
    if view.importance != Importance::Standard {
        column = column.child(importance_tag(view.importance, &theme));
    }
    if channel && !reply_open {
        let subject = this.ui.composer.subject.clone();
        column = column.child(
            div()
                .py(px(3.))
                .text_size(px(13.))
                .line_height(px(18.))
                .text_color(theme.text)
                .child(field(&subject, widgets::input_colors(&theme), cx)),
        );
    }
    column = column.child(editor(this, 140., cx));
    if let Some(preview) = view.link_preview.clone() {
        column = column.child(link_preview(this, &preview, cx));
    }
    if !view.cards.is_empty() {
        column = column.child(composer_extensions::cards(this, &view.cards, window, cx));
    }
    // Pasted images show in the text; a failed upload needs its chip to say so.
    let chips: Vec<_> = view
        .attachments
        .iter()
        .filter(|a| !a.inline || a.error.is_some())
        .cloned()
        .collect();
    if !chips.is_empty() {
        column = column.child(crate::files::attachment_chips(this, &chips, cx));
    }
    column = column.child(tools(this, &view, thread, channel, window, cx));
    let mention = composer_mention::picker(this, &this.ui.composer.clone_handles(), cx);

    div()
        .w_full()
        .flex_none()
        .pb(px(16.))
        .px(px(20.))
        .child(
            div()
                .id("composer-box")
                .pt(px(10.))
                .px(px(12.))
                .pb(px(8.))
                .rounded(px(12.))
                .bg(theme.surface)
                .border_1()
                .border_color(theme.border_strong)
                .drag_over::<ExternalPaths>(move |style, _, _, _| {
                    style.border_2().border_color(theme.accent)
                })
                .on_drop(cx.listener(|this, paths: &ExternalPaths, _, _| {
                    let paths: Vec<String> = paths
                        .paths()
                        .iter()
                        .map(|p| p.to_string_lossy().into_owned())
                        .collect();
                    if !paths.is_empty() {
                        this.send(Action::Files {
                            action: FileAction::Attach { paths },
                        });
                    }
                }))
                .child(column),
        )
        .children(mention)
        .into_any_element()
}

/// Shared by the composer and the message editor before drawing: placeholder, Return's role, focus.
fn prepare(this: &mut Oxide, placeholder: String, window: &mut Window, cx: &mut Context<Oxide>) {
    let formatting = this.ui.composer.formatting;
    let ui = &mut this.ui.composer;
    let focus = std::mem::take(&mut ui.focus_wanted);
    let refocus = std::mem::take(&mut ui.refocus) && window.focused(cx).is_none();
    ui.input.update(cx, |input, _| {
        input.set_placeholder(placeholder);
        // With the formatting tools open, Return breaks the line and ⌘ Return sends (`expanded` in Swift).
        input.set_submits(!formatting);
    });
    if focus || refocus {
        let input = ui.input.clone();
        input.update(cx, |input, cx| {
            input.focus(window);
            if focus {
                let end = input.text().len();
                input.select(end..end, cx);
            }
        });
    }
    // A caret moved by the keyboard or mouse: the mention typed there, if any.
    let state = this.ui.composer.input.read(cx);
    let selection = state.selection();
    if !this.ui.composer.mentions.seen(state.text(), &selection) {
        capture(this, false, cx);
    }
}

impl Ui {
    /// What the mention list needs, without borrowing the whole store.
    fn clone_handles(&self) -> Ui2 {
        Ui2 {
            input: self.input.clone(),
            mention: self.mentions.current.clone(),
            highlighted: self.mentions.highlighted,
        }
    }
}
/// See [`Ui::clone_handles`].
pub struct Ui2 {
    pub input: Entity<TextInput>,
    pub mention: Option<composer_mention::MentionQuery>,
    pub highlighted: usize,
}

/// "IMPORTANT!" or "URGENT!" above the draft, as on a message.
pub fn importance_tag(importance: Importance, theme: &Theme) -> AnyElement {
    let (glyph, label) = match importance {
        Importance::Urgent => (Icon::Bell, tr("ui.composer.urgent_tag")),
        _ => (Icon::Alert, tr("ui.composer.important_tag")),
    };
    div()
        .flex()
        .items_center()
        .gap_1()
        .text_size(px(11.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(theme.error_text)
        .child(icon(glyph, px(12.), theme.error_text))
        .child(label)
        .into_any_element()
}

/// The row of tools under the editor, the send status and the send button.
fn tools(
    this: &mut Oxide,
    view: &ComposerView,
    thread: bool,
    channel: bool,
    window: &mut Window,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    let theme = this.theme;
    let formatting = this.ui.composer.formatting;
    let ready = view.attachments.iter().all(|a| a.ready);
    let can_send = !view.sending
        && (!view.draft.trim().is_empty()
            || !view.attachments.is_empty()
            || !view.cards.is_empty())
        && ready;
    let format_toggle = format_toggle(formatting, &theme, cx);
    let emoji = emoji_button(this, window, cx);
    let fun = with_popover(
        this,
        FUN,
        text_tool("composer-gif", "GIF", 11., this.popup_open(FUN), &theme)
            .tooltip(widgets::tooltip(
                tr("ui.fun_picker.gifs_and_stickers"),
                theme,
            ))
            .on_click(cx.listener(|this, _, _, cx| toggle(this, FUN, cx))),
        composer_fun::picker,
        window,
        cx,
    );
    let apps = with_popover(
        this,
        APPS,
        widgets::icon_button_sized(
            "composer-apps",
            Icon::Apps,
            32.,
            16.,
            this.popup_open(APPS),
            &theme,
        )
        .tooltip(widgets::tooltip(tr("ui.extensions.apps"), theme))
        .on_click(cx.listener(|this, _, _, cx| toggle(this, APPS, cx))),
        composer_extensions::flyout,
        window,
        cx,
    );
    let mention = mention_button(&theme, cx);
    let attach = widgets::icon_button_sized("composer-attach", Icon::Clip, 32., 16., false, &theme)
        .tooltip(widgets::tooltip(tr("ui.composer.attach_files"), theme))
        .on_click(cx.listener(|_, _, _, cx| choose_files(cx)));
    let delivery = delivery_options(this, view.importance, channel, window, cx);
    let status = if thread {
        compact_status(this, view, &theme)
    } else {
        send_status(this, view, formatting, &theme)
    };
    let send = widgets::disabled(
        div()
            .id("composer-send")
            .w(px(36.))
            .h(px(32.))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(7.))
            .bg(theme.accent)
            .cursor_pointer()
            .when(can_send, |d| d.hover(|s| s.opacity(0.9)))
            .child(icon(Icon::Send, px(16.), gpui::white())),
        !can_send,
    )
    .tooltip(widgets::tooltip(
        if formatting {
            tr("ui.composer.send_enter")
        } else {
            tr("ui.composer.send_shift_enter_for_a_new")
        },
        theme,
    ))
    .when(can_send, |d| {
        d.on_click(cx.listener(|this, _, _, _| this.send(Action::Send)))
    });
    div()
        .flex()
        .items_center()
        .gap(px(2.))
        .child(format_toggle)
        .child(emoji)
        .child(fun)
        .child(apps)
        .child(mention)
        .child(attach)
        .child(delivery)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .justify_end()
                .pr(px(8.))
                .child(status),
        )
        .child(send)
        .into_any_element()
}

pub fn format_toggle(formatting: bool, theme: &Theme, cx: &mut Context<Oxide>) -> AnyElement {
    text_tool("composer-format", "Aa", 13., formatting, theme)
        .tooltip(widgets::tooltip(
            if formatting {
                tr("ui.composer.hide_formatting_options")
            } else {
                tr("ui.composer.show_formatting_options")
            },
            *theme,
        ))
        .on_click(cx.listener(|this, _, window, cx| {
            this.ui.composer.formatting = !this.ui.composer.formatting;
            this.ui.composer.input.read(cx).focus(window);
            cx.notify();
        }))
        .into_any_element()
}

pub fn emoji_button(this: &mut Oxide, window: &mut Window, cx: &mut Context<Oxide>) -> AnyElement {
    let theme = this.theme;
    with_popover(
        this,
        EMOJI,
        widgets::icon_button_sized(
            "composer-emoji",
            Icon::Smile,
            32.,
            16.,
            this.popup_open(EMOJI),
            &theme,
        )
        .tooltip(widgets::tooltip(tr("ui.composer.add_an_emoji"), theme))
        .on_click(cx.listener(|this, _, _, cx| toggle(this, EMOJI, cx))),
        composer_emoji::picker,
        window,
        cx,
    )
}

/// Starts a mention at the caret, apart from the word before it.
pub fn mention_button(theme: &Theme, cx: &mut Context<Oxide>) -> AnyElement {
    text_tool("composer-mention", "@", 15., false, theme)
        .tooltip(widgets::tooltip(
            tr("ui.composer.mention_someone_2"),
            *theme,
        ))
        .on_click(cx.listener(|this, _, window, cx| {
            start_mention(this, cx);
            this.ui.composer.input.read(cx).focus(window);
        }))
        .into_any_element()
}

pub fn start_mention(this: &mut Oxide, cx: &mut Context<Oxide>) {
    let input = this.ui.composer.input.read(cx);
    let before = input.text()[..input.selection().start].chars().last();
    let prefix = if before.is_some_and(|c| !c.is_whitespace() && !"([{>".contains(c)) {
        " "
    } else {
        ""
    };
    replace(this, &format!("{prefix}@"), None, None, None, cx);
}

/// The open panel to pick files to attach.
fn choose_files(cx: &mut Context<Oxide>) {
    let chosen = cx.prompt_for_paths(PathPromptOptions {
        files: true,
        directories: false,
        multiple: true,
        prompt: Some(tr("ui.composer.attach").into()),
    });
    cx.spawn(async move |this, cx| {
        let Ok(Ok(Some(paths))) = chosen.await else {
            return;
        };
        let paths: Vec<String> = paths
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        if !paths.is_empty() {
            let _ = this.update(cx, |this, _| {
                this.send(Action::Files {
                    action: FileAction::Attach { paths },
                })
            });
        }
    })
    .detach();
}

/// "Set delivery options": Standard, Important, and Urgent in chats only.
fn delivery_options(
    this: &mut Oxide,
    importance: Importance,
    channel: bool,
    window: &mut Window,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    let theme = this.theme;
    let color = if importance == Importance::Standard {
        theme.muted
    } else {
        theme.error_text
    };
    let hover = theme.input;
    let button = div()
        .id("composer-importance")
        .size(px(32.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(7.))
        .cursor_pointer()
        .hover(move |s| s.bg(hover))
        .child(icon(Icon::Alert, px(16.), color))
        .tooltip(widgets::tooltip(tr("ui.composer.delivery_options"), theme))
        .on_click(cx.listener(|this, _, _, cx| toggle(this, IMPORTANCE, cx)));
    with_popover(
        this,
        IMPORTANCE,
        button,
        move |this, _, cx| {
            let theme = this.theme;
            let mut options = vec![
                (Importance::Standard, tr("ui.composer.standard")),
                (Importance::High, tr("ui.composer.important")),
            ];
            if !channel {
                options.push((Importance::Urgent, tr("ui.composer.urgent")));
            }
            let mut menu = div().py_1().flex().flex_col().child(
                div()
                    .px_3()
                    .py_1()
                    .text_size(px(11.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.muted)
                    .child(tr("ui.composer.delivery_options")),
            );
            for (ix, (value, label)) in options.into_iter().enumerate() {
                let chosen = value == importance;
                menu = menu.child(
                    widgets::menu_item(
                        ("importance", ix),
                        chosen.then_some(Icon::Check),
                        label,
                        &theme,
                    )
                    .when(!chosen, |d| d.pl(px(31.)))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.send(Action::SetImportance { importance: value });
                        this.close_popup(cx);
                        this.ui.composer.input.read(cx).focus(window);
                    })),
                );
            }
            menu.into_any_element()
        },
        window,
        cx,
    )
}

/// What happens to the draft: being sent, waiting for the network or uploads, or how to send it.
fn send_status(this: &Oxide, view: &ComposerView, formatting: bool, theme: &Theme) -> AnyElement {
    let small = |text: String, color| {
        div()
            .text_size(px(11.))
            .text_color(color)
            .whitespace_nowrap()
            .child(text)
    };
    if view.sending {
        div()
            .flex()
            .items_center()
            .gap(px(6.))
            .child(widgets::spinner("composer-sending", 12., theme.muted))
            .child(small(tr("ui.composer.sending"), theme.muted).font_weight(FontWeight::MEDIUM))
            .into_any_element()
    } else if !this.online {
        div()
            .flex()
            .items_center()
            .gap(px(4.))
            .child(icon(Icon::WifiOff, px(12.), theme.error_text))
            .child(
                small(tr("ui.composer.offline_can_t_send"), theme.error_text)
                    .font_weight(FontWeight::MEDIUM),
            )
            .into_any_element()
    } else if !view.attachments.iter().all(|a| a.ready) {
        small(tr("ui.composer.you_can_send_once_the_files"), theme.subtle).into_any_element()
    } else {
        small(
            if formatting {
                tr("ui.composer.enter_to_send")
            } else {
                tr("ui.composer.enter_to_send_2")
            },
            theme.subtle,
        )
        .into_any_element()
    }
}

/// `send_status` without words, for the narrow thread pane.
fn compact_status(this: &Oxide, view: &ComposerView, theme: &Theme) -> AnyElement {
    if view.sending {
        div()
            .id("composer-sending-compact")
            .tooltip(widgets::tooltip(tr("ui.composer.sending"), *theme))
            .child(widgets::spinner("composer-sending", 12., theme.muted))
            .into_any_element()
    } else if !this.online {
        div()
            .id("composer-offline")
            .tooltip(widgets::tooltip(
                tr("ui.composer.offline_can_t_send"),
                *theme,
            ))
            .child(icon(Icon::WifiOff, px(14.), theme.error_text))
            .into_any_element()
    } else {
        div().into_any_element()
    }
}

/// The draft's link preview, as a single row that can be closed; a click opens the link.
fn link_preview(
    this: &mut Oxide,
    preview: &teams_core::LinkPreviewView,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    let theme = this.theme;
    let url = preview.url.clone();
    let picture: Option<AnyElement> = if preview.loading {
        Some(
            div()
                .size(px(56.))
                .flex()
                .items_center()
                .justify_center()
                .child(widgets::spinner("preview-loading", 14., theme.muted))
                .into_any_element(),
        )
    } else {
        preview.image.as_ref().map(|image| {
            match (&image.status, this.images.images.get(&image.src)) {
                (teams_core::ImageStatus::Ready { .. }, Some(picture)) => img(picture.clone())
                    .size(px(56.))
                    .rounded(px(6.))
                    .object_fit(gpui::ObjectFit::Cover)
                    .into_any_element(),
                (status, _) => {
                    let src = image.src.clone();
                    let box_ = div()
                        .id("preview-image")
                        .size(px(56.))
                        .rounded(px(6.))
                        .bg(theme.input)
                        .flex()
                        .items_center()
                        .justify_center();
                    match status {
                        teams_core::ImageStatus::Failed { reason } => box_
                            .cursor_pointer()
                            .tooltip(widgets::tooltip(
                                format!("{reason} · {}", tr("ui.message_row.retry_image")),
                                theme,
                            ))
                            .child(icon(Icon::Image, px(16.), theme.muted))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                cx.stop_propagation();
                                this.send(Action::RetryImage { src: src.clone() });
                            }))
                            .into_any_element(),
                        _ => box_
                            .child(widgets::spinner("preview-image-loading", 12., theme.muted))
                            .into_any_element(),
                    }
                }
            }
        })
    };
    let texts = div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(3.))
        .when_some(preview.title.clone(), |d, title| {
            d.child(
                div()
                    .text_size(px(13.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .line_clamp(2)
                    .child(title),
            )
        })
        .when_some(preview.description.clone(), |d, text| {
            d.child(
                div()
                    .text_size(px(13.))
                    .text_color(theme.muted)
                    .truncate()
                    .child(text),
            )
        })
        .child(
            div()
                .flex()
                .items_center()
                .gap_1()
                .child(icon(Icon::Link, px(10.), theme.muted))
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(theme.muted)
                        .truncate()
                        .child(preview.site.clone()),
                ),
        );
    div()
        .id("composer-link-preview")
        .flex()
        .items_start()
        .gap(px(10.))
        .p(px(8.))
        .rounded(px(8.))
        .bg(theme.input)
        .cursor_pointer()
        .tooltip(widgets::tooltip(preview.url.clone(), theme))
        .on_click(cx.listener(move |this, _, _, _| this.send(Action::OpenUrl { url: url.clone() })))
        .children(picture)
        .child(texts)
        .child(
            widgets::icon_button_sized("preview-dismiss", Icon::Close, 22., 10., false, &theme)
                .tooltip(widgets::tooltip(
                    tr("ui.link_preview.remove_preview"),
                    theme,
                ))
                .on_click(cx.listener(|this, _, _, cx| {
                    cx.stop_propagation();
                    this.send(Action::LinkPreview {
                        action: LinkPreviewAction::Dismiss,
                    });
                })),
        )
        .into_any_element()
}

/// My message in the composer's rich editor, inside its bubble: Return saves, Shift-Return breaks the line,
/// Escape cancels (`MessageRow.swift`'s `MessageEditor`). For the feed row of the message being edited.
pub fn message_editor(
    this: &mut Oxide,
    id: &str,
    window: &mut Window,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    let theme = this.theme;
    if this.ui.composer.editing.as_deref() != Some(id) {
        this.ui.composer.editing = Some(id.to_owned());
        this.ui.composer.focus_wanted = true;
    }
    prepare(this, tr("ui.message_row.edit_message"), window, cx);
    let formatting = this.ui.composer.formatting;
    let view = this.composer.clone().unwrap_or_else(empty_view);
    let mut column = div().w_full().flex().flex_col().gap(px(8.));
    if formatting {
        column = column.child(composer_toolbar::toolbar(this, window, cx));
    }
    let editor = editor(this, 300., cx);
    column = column.child(
        div()
            .px(px(8.))
            .py(px(4.))
            .rounded(px(8.))
            .bg(theme.surface)
            .border_1()
            .border_color(theme.border_strong)
            .child(editor),
    );
    let failed: Vec<_> = view
        .attachments
        .iter()
        .filter(|a| a.error.is_some())
        .cloned()
        .collect();
    if !failed.is_empty() {
        column = column.child(crate::files::attachment_chips(this, &failed, cx));
    }
    let toggle = format_toggle(formatting, &theme, cx);
    let emoji = emoji_button(this, window, cx);
    let mention = mention_button(&theme, cx);
    column = column.child(
        div()
            .flex()
            .items_center()
            .gap_2()
            .child(
                div()
                    .flex()
                    .gap(px(2.))
                    .child(toggle)
                    .child(emoji)
                    .child(mention),
            )
            .child(
                widgets::button(
                    "edit-save",
                    tr("ui.message_row.save"),
                    ButtonKind::Primary,
                    &theme,
                )
                .on_click(cx.listener(|this, _, _, _| this.send(Action::SaveEdit))),
            )
            .child(
                widgets::button(
                    "edit-cancel",
                    tr("ui.message_row.cancel"),
                    ButtonKind::Outline,
                    &theme,
                )
                .on_click(cx.listener(|this, _, _, _| this.send(Action::CancelEdit))),
            )
            .child(
                div()
                    .text_size(px(11.))
                    .text_color(theme.muted)
                    .child(if formatting {
                        tr("ui.message_row.enter_to_save")
                    } else {
                        tr("ui.message_row.enter_to_save_enter_for_a")
                    }),
            ),
    );
    let mention = composer_mention::picker(this, &this.ui.composer.clone_handles(), cx);
    column.children(mention).into_any_element()
}

/// Dialogs and floating panels of this module, drawn over the window: the link editor.
pub fn overlay(
    this: &mut Oxide,
    window: &mut Window,
    cx: &mut Context<Oxide>,
) -> Option<AnyElement> {
    // Pictures of the pickers are kept only while one is open.
    if ![EMOJI, FUN, APPS].iter().any(|k| this.popup_open(k)) {
        this.ui.composer.thumbs.clear();
    }
    if !this.popup_open(EMOJI) {
        composer_emoji::closed(this);
    }
    if this.shell.as_ref().is_none_or(|s| s.editing.is_none()) {
        this.ui.composer.editing = None;
    }
    composer_toolbar::overlay(this, window, cx)
}

/// Steps of `OXIDE_SCRIPT`: `composer:type <text>`, `composer:focus`, `composer:format`, `composer:select <a>-<b>`,
/// `composer:bold`, `composer:block <bullet|numbered|quote|h1|code>`, `composer:emoji`, `composer:emoji-search <q>`,
/// `composer:fun`, `composer:stickers`, `composer:apps`, `composer:app <n>`, `composer:importance`,
/// `composer:mention`, `composer:link`, `composer:send`, `composer:edit-last`, `composer:popup <key>`.
pub fn debug_step(
    this: &mut Oxide,
    step: &str,
    window: &mut Window,
    cx: &mut Context<Oxide>,
) -> bool {
    let Some(rest) = step.strip_prefix("composer:") else {
        return false;
    };
    let (verb, arg) = rest.split_once(' ').unwrap_or((rest, ""));
    let input = this.ui.composer.input.clone();
    match verb {
        "focus" => focus(this, cx),
        "type" => {
            input.update(cx, |i, cx| {
                i.focus(window);
                i.insert(&arg.replace("\\n", "\n"), window, cx)
            });
        }
        "format" => this.ui.composer.formatting = !this.ui.composer.formatting,
        "select" => {
            let (a, b) = arg.split_once('-').unwrap_or((arg, arg));
            let (a, b) = (a.parse().unwrap_or(0), b.parse().unwrap_or(0));
            input.update(cx, |i, cx| i.select(a..b, cx));
        }
        "bold" => input.update(cx, |i, cx| {
            let on = !i.current_format().bold;
            i.change_format(false, |f| f.bold = on, cx)
        }),
        "block" => {
            let block = match arg {
                "bullet" => teams_core::ComposerBlock::Bullet,
                "numbered" => teams_core::ComposerBlock::Numbered,
                "quote" => teams_core::ComposerBlock::Quote,
                "h1" => teams_core::ComposerBlock::Heading1,
                "code" => teams_core::ComposerBlock::CodeBlock,
                _ => teams_core::ComposerBlock::Paragraph,
            };
            composer_toolbar::set_block(this, block, window, cx);
        }
        "paragraphs" => {
            // One block per paragraph, in order: `bullet,numbered,quote,code,h1,-`.
            for (ix, name) in arg.split(',').enumerate() {
                let block = match name {
                    "bullet" => teams_core::ComposerBlock::Bullet,
                    "numbered" => teams_core::ComposerBlock::Numbered,
                    "quote" => teams_core::ComposerBlock::Quote,
                    "h1" => teams_core::ComposerBlock::Heading1,
                    "code" => teams_core::ComposerBlock::CodeBlock,
                    _ => continue,
                };
                let start = input
                    .read(cx)
                    .text()
                    .split('\n')
                    .take(ix)
                    .map(|p| p.len() + 1)
                    .sum::<usize>();
                input.update(cx, |i, cx| i.select(start..start, cx));
                composer_toolbar::set_block(this, block, window, cx);
            }
            let end = input.read(cx).text().len();
            input.update(cx, |i, cx| i.select(end..end, cx));
        }
        "emoji" => toggle(this, EMOJI, cx),
        "emoji-search" => composer_emoji::search(this, arg, window, cx),
        "fun" => toggle(this, FUN, cx),
        "stickers" => composer_fun::show_stickers(this, cx),
        "apps" => toggle(this, APPS, cx),
        "app" => composer_extensions::debug_choose(this, arg.parse().unwrap_or(0)),
        "importance" => toggle(this, IMPORTANCE, cx),
        "mention" => start_mention(this, cx),
        "link" => composer_toolbar::request_link(this, window, cx),
        "send" => submit(this),
        "edit-last" => {
            edit_last(this);
        }
        "popup" => {
            this.popup = Some(arg.to_owned().into());
        }
        "dump" => {
            let input = input.read(cx);
            eprintln!(
                "composer: chat={:?} core={:?} formatting={} text={:?} selection={:?} formats={:?} mention={:?}",
                this.conversation.as_ref().map(|c| c.title.clone()),
                this.composer
                    .as_ref()
                    .map(|c| (c.draft_rev, c.draft.clone())),
                this.ui.composer.formatting,
                input.text(),
                input.selection(),
                input.formats(),
                this.ui.composer.mentions.current
            );
        }
        _ => return false,
    }
    cx.notify();
    true
}

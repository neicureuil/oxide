//! One message of a feed (`MessageRow.swift`): author line, hover actions, bubble or flat body, reactions,
//! its menu, the in-place editor and the delete confirmation.
use crate::feed;
use crate::feed_blocks::{self, Caption};
use crate::icons::{Icon, icon};
use crate::oxide::{Oxide, tr, trf};
use crate::widgets::{self, ButtonKind};
use gpui::{
    AnyElement, ClipboardItem, Context, Corner, Div, FontWeight, MouseButton, MouseDownEvent,
    SharedString, Window, div, prelude::*, px,
};
use teams_core::{Action, ComposerBlock, ComposerFormat, Importance, MessageView, ReceiptView};

/// Where a message sits, which says how it is drawn and what "reply" does.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum RowKind {
    /// A chat message: a bubble, mine on the right; reply quotes it in the composer.
    Bubble,
    /// A thread reply: flat with its author; reply quotes it.
    Reply,
    /// The root of a channel post (its id): flat; reply opens its thread.
    Post(String),
}

/// The right-click menu: which message, how it is drawn, and where the click was.
#[derive(Clone)]
pub struct MessageMenu {
    pub message: MessageView,
    pub kind: RowKind,
    pub at: gpui::Point<gpui::Pixels>,
}

pub const MENU_KEY: &str = "msg-menu";
fn react_key(id: &str) -> String {
    format!("react:{id}")
}
fn more_key(id: &str) -> String {
    format!("msg-more:{id}")
}

fn plain_text(msg: &MessageView) -> String {
    feed_blocks::body_text(&msg.blocks)
}

/// UTF-16 length, the unit of the core's draft offsets.
fn utf16_len(text: &str) -> u32 {
    text.encode_utf16().count() as u32
}

/// The draft edits opening a reply: the quoted line, then a plain line break the core's caret ends after (`Store.replyEdits`).
pub fn reply_edits(line: &str) -> [Action; 2] {
    let empty = ComposerFormat {
        alignment: "left".into(),
        ..Default::default()
    };
    let quote = ComposerFormat {
        block: ComposerBlock::Quote,
        ..empty.clone()
    };
    [
        Action::ReplaceDraft {
            start: 0,
            length: 0,
            text: line.to_owned(),
            mention: None,
            emoji: None,
            style: Some(quote),
        },
        Action::ReplaceDraft {
            start: utf16_len(line),
            length: 0,
            text: "\n".into(),
            mention: None,
            emoji: None,
            style: Some(empty),
        },
    ]
}

/// « Author : excerpt », the quoted line of a reply (`Store.quote`).
pub fn quote_line(msg: &MessageView) -> String {
    let words = plain_text(msg)
        .split(['\n', '\r'])
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    let excerpt = if words.chars().count() > 160 {
        format!("{}…", words.chars().take(160).collect::<String>())
    } else {
        words
    };
    let author = if msg.mine {
        tr("ui.store.you")
    } else {
        msg.sender.clone()
    };
    trf(
        "ui.store.text",
        &[("author", &author), ("excerpt", &excerpt)],
    )
}

/// Answers a message: opens the post's thread, or quotes it in the draft.
fn reply(this: &mut Oxide, msg: &MessageView, kind: &RowKind, cx: &mut Context<Oxide>) {
    match kind {
        RowKind::Post(post) => this.send(Action::OpenReply { post: post.clone() }),
        RowKind::Bubble | RowKind::Reply => {
            for action in reply_edits(&quote_line(msg)) {
                this.send(action);
            }
        }
    }
    crate::composer::focus(this, cx);
}

pub fn message_row(
    this: &mut Oxide,
    msg: &MessageView,
    kind: RowKind,
    window: &mut Window,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    let row = message_row_inner(this, msg, kind, window, cx);
    // A message that just arrived fades in and settles, as the Swift table inserts rows.
    if crate::feed::is_fresh(&msg.id) {
        return crate::motion::animate(
            gpui::div().w_full().child(row),
            SharedString::from(format!("fresh-{}", msg.id)),
            crate::motion::Curve::Standard,
            |d, t| d.opacity(t).mt(px(6. * (1. - t))),
        );
    }
    row
}

fn message_row_inner(
    this: &mut Oxide,
    msg: &MessageView,
    kind: RowKind,
    window: &mut Window,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    let bubble = kind == RowKind::Bubble;
    let meta = meta(this, msg, &kind, bubble, window, cx);
    let reactions = reaction_bar(this, msg);
    let row = if !bubble {
        let avatar = this.avatar(
            &msg.sender_id,
            &msg.sender,
            32.,
            msg.sender_avatar_shape,
            msg.sender_presence.as_ref(),
        );
        div()
            .w_full()
            .flex()
            .items_start()
            .gap(px(10.))
            .child(avatar)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(meta)
                    .child(content(this, msg, false, window, cx))
                    .children(reactions),
            )
    } else if msg.mine {
        div().w_full().flex().justify_end().pl(px(80.)).child(
            div()
                .max_w(px(560.))
                .min_w_0()
                .flex()
                .flex_col()
                .items_end()
                .gap(px(4.))
                .child(meta)
                .child(bubble_card(this, msg, window, cx))
                .children(reactions),
        )
    } else {
        let avatar = this.avatar(
            &msg.sender_id,
            &msg.sender,
            32.,
            msg.sender_avatar_shape,
            msg.sender_presence.as_ref(),
        );
        div()
            .w_full()
            .flex()
            .items_start()
            .gap(px(10.))
            .pr(px(40.))
            .child(avatar)
            .child(
                div()
                    .max_w(px(600.))
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .items_start()
                    .gap(px(4.))
                    .child(meta)
                    .child(bubble_card(this, msg, window, cx))
                    .children(reactions),
            )
    };
    let id = msg.id.clone();
    let hover_id = msg.id.clone();
    let menu_message = msg.clone();
    let editing = msg.editing;
    div()
        .id(SharedString::from(format!("message-{id}")))
        .w_full()
        .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
            let ui = &mut this.ui.conversation;
            if *hovered {
                if ui.hovered.as_deref() != Some(hover_id.as_str()) {
                    ui.hovered = Some(hover_id.clone());
                    cx.notify();
                }
            } else if ui.hovered.as_deref() == Some(hover_id.as_str()) {
                ui.hovered = None;
                cx.notify();
            }
        }))
        .when(!editing, |d| {
            d.on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    this.ui.conversation.menu = Some(MessageMenu {
                        message: menu_message.clone(),
                        kind: kind.clone(),
                        at: event.position,
                    });
                    this.popup = Some(MENU_KEY.into());
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
        })
        .child(row)
        .into_any_element()
}

fn bubble_card(
    this: &mut Oxide,
    msg: &MessageView,
    window: &mut Window,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    let theme = this.theme;
    let (big, small) = (px(12.), px(4.));
    let bubble_width: f32 = if msg.mine { 560. } else { 600. };
    // Pictures and cards inside fit the bubble, not only the feed: wider, they ran out of its side.
    let inside = bubble_width - 2. * BUBBLE_PADDING - if msg.mine { 0. } else { 2. };
    let body = crate::feed::in_bubble(inside, || content(this, msg, true, window, cx));
    div()
        .min_w_0()
        // Also on the bubble itself, not only its column: the list measures text against the widest it may be,
        // and a long paragraph measured wider than it's drawn overlapped the rows below.
        .max_w(px(bubble_width))
        .px(px(BUBBLE_PADDING))
        .py(px(10.))
        .bg(if msg.mine {
            theme.mine_bubble
        } else {
            theme.surface
        })
        .when(!msg.mine, |d| {
            d.border_1().border_color(theme.bubble_border)
        })
        .rounded_bl(big)
        .rounded_br(big)
        .rounded_tl(if msg.mine { big } else { small })
        .rounded_tr(if msg.mine { small } else { big })
        .child(body)
        .into_any_element()
}

/// Space between a bubble's edge and its content.
const BUBBLE_PADDING: f32 = 14.;

/// « **Name** · 20:12 » (only the time on my bubbles), with the actions beside it so showing them never moves the feed.
fn meta(
    this: &mut Oxide,
    msg: &MessageView,
    kind: &RowKind,
    bubble: bool,
    window: &mut Window,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    let theme = this.theme;
    let time = div()
        .text_size(px(12.))
        .text_color(theme.muted)
        .child(msg.time.clone());
    let author = if msg.mine && bubble {
        time.into_any_element()
    } else {
        // Name and time as one selectable line, so a copy across messages keeps who said what.
        let name = if msg.mine {
            tr("ui.message_row.you")
        } else {
            msg.sender.clone()
        };
        div()
            .min_w_0()
            .text_size(px(if bubble { 12. } else { 13. }))
            .text_color(theme.muted)
            .child(crate::feed_select::author_line(
                &msg.id,
                name,
                format!(" · {}", msg.time),
                gpui::HighlightStyle {
                    color: Some(theme.text),
                    font_weight: Some(FontWeight::SEMIBOLD),
                    ..Default::default()
                },
                gpui::HighlightStyle {
                    color: Some(theme.muted),
                    ..Default::default()
                },
                theme.accent.opacity(0.3),
            ))
            .into_any_element()
    };
    let actions = actions(this, msg, kind, window, cx);
    let row = div().h(px(24.)).flex().items_center().gap_2();
    if msg.mine && bubble {
        row.children(actions)
            .child(author)
            .children(msg.receipt.map(|r| receipt(r, this, &msg.id)))
            .into_any_element()
    } else {
        row.child(author).children(actions).into_any_element()
    }
}

/// Teams' status of my latest messages: a check for sent, an eye once read.
fn receipt(receipt: ReceiptView, this: &Oxide, id: &str) -> AnyElement {
    let theme = this.theme;
    let (glyph, label) = match receipt {
        ReceiptView::Sent => (Icon::Check, tr("ui.typing_indicator.sent")),
        ReceiptView::Seen => (Icon::Eye, tr("ui.typing_indicator.seen")),
        ReceiptView::SeenByEveryone => (Icon::Eye, tr("ui.typing_indicator.seen_by_everyone")),
    };
    let color = if receipt == ReceiptView::Sent {
        theme.subtle
    } else {
        theme.accent
    };
    div()
        .id(SharedString::from(format!("receipt-{id}")))
        .child(icon(glyph, px(12.), color))
        .tooltip(widgets::tooltip(label, theme))
        .into_any_element()
}

fn small_button(id: String, glyph: Icon, help: String, this: &Oxide) -> gpui::Stateful<Div> {
    let theme = this.theme;
    widgets::icon_button_sized(SharedString::from(id), glyph, 24., 13., false, &theme)
        .rounded(px(8.))
        .tooltip(widgets::tooltip(help, theme))
}

/// The hover bar: react, reply, edit, delete and the "more" menu.
fn actions(
    this: &mut Oxide,
    msg: &MessageView,
    kind: &RowKind,
    window: &mut Window,
    cx: &mut Context<Oxide>,
) -> Option<AnyElement> {
    let theme = this.theme;
    let react_open = this.popup_open(&react_key(&msg.id));
    let more_open = this.popup_open(&more_key(&msg.id));
    let hovered = this.ui.conversation.hovered.as_deref() == Some(msg.id.as_str())
        || this.ui.conversation.debug_hover_all;
    let visible = hovered || msg.pending || react_open || more_open;
    if !visible || msg.deleted {
        return None;
    }
    let can_edit = msg.editable && !msg.editing;
    let mut bar = div().flex().items_center().gap(px(2.));
    if msg.can_react || msg.pending {
        let id = msg.id.clone();
        let button = if msg.pending {
            div()
                .id(SharedString::from(format!("react-pending-{}", msg.id)))
                .size(px(24.))
                .flex()
                .items_center()
                .justify_center()
                .child(widgets::spinner(
                    SharedString::from(format!("react-spin-{}", msg.id)),
                    12.,
                    theme.muted,
                ))
                .tooltip(widgets::tooltip(
                    tr("ui.message_row.sending_reaction_2"),
                    theme,
                ))
        } else {
            small_button(
                format!("react-{}", msg.id),
                Icon::Smile,
                tr("ui.message_row.react"),
                this,
            )
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_popup(react_key(&id), cx)))
        };
        bar = bar.child(div().relative().child(button).when(react_open, |d| {
            d.child(crate::feed_react::popover(this, msg, window, cx))
        }));
    }
    if !msg.pending {
        let message = msg.clone();
        let kind = kind.clone();
        bar = bar.child(
            small_button(
                format!("reply-{}", msg.id),
                Icon::Reply,
                tr("ui.message_row.reply"),
                this,
            )
            .on_click(cx.listener(move |this, _, _, cx| reply(this, &message, &kind, cx))),
        );
    }
    if can_edit {
        let id = msg.id.clone();
        let delete_id = msg.id.clone();
        bar = bar
            .child(
                small_button(
                    format!("edit-{}", msg.id),
                    Icon::Pencil,
                    tr("ui.message_row.edit"),
                    this,
                )
                .on_click(
                    cx.listener(move |this, _, _, _| this.send(Action::Edit { id: id.clone() })),
                ),
            )
            .child(
                small_button(
                    format!("delete-{}", msg.id),
                    Icon::Trash,
                    tr("ui.message_row.delete"),
                    this,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.ui.conversation.confirm_delete = Some(delete_id.clone());
                    cx.notify();
                })),
            );
    }
    if !msg.editing {
        let id = msg.id.clone();
        let items = menu_items(this, msg, kind, cx);
        bar = bar.child(
            div()
                .relative()
                .child(
                    small_button(
                        format!("more-{}", msg.id),
                        Icon::More,
                        tr("ui.message_row.more_actions"),
                        this,
                    )
                    .on_click(
                        cx.listener(move |this, _, _, cx| this.toggle_popup(more_key(&id), cx)),
                    ),
                )
                .when(more_open, |d| {
                    let close = cx.listener(|this, _: &(), _, cx| this.close_popup(cx));
                    d.child(widgets::popover(
                        Corner::TopLeft,
                        div().flex().flex_col().children(items),
                        &theme,
                        move |w, cx| close(&(), w, cx),
                    ))
                }),
        );
    }
    Some(bar.into_any_element())
}

/// Everything the hover bar offers and more, for its "more" button and the right click.
pub fn menu_items(
    this: &Oxide,
    msg: &MessageView,
    kind: &RowKind,
    cx: &mut Context<Oxide>,
) -> Vec<AnyElement> {
    let theme = this.theme;
    let mut items = Vec::new();
    if msg.deleted {
        return items;
    }
    let close_then = |f: Box<dyn Fn(&mut Oxide, &mut Context<Oxide>)>| {
        cx.listener(move |this: &mut Oxide, _: &gpui::ClickEvent, _, cx| {
            this.popup = None;
            f(this, cx);
            cx.notify();
        })
    };
    if msg.can_react {
        let id = msg.id.clone();
        items.push(
            widgets::menu_item(
                "menu-react",
                Some(Icon::Smile),
                tr("ui.message_row.react"),
                &theme,
            )
            .on_click(close_then(Box::new(move |this, _| {
                this.popup = Some(react_key(&id).into())
            })))
            .into_any_element(),
        );
    }
    {
        let (message, kind) = (msg.clone(), kind.clone());
        items.push(
            widgets::menu_item(
                "menu-reply",
                Some(Icon::Reply),
                tr("ui.message_row.reply"),
                &theme,
            )
            .on_click(close_then(Box::new(move |this, cx| {
                reply(this, &message, &kind, cx)
            })))
            .into_any_element(),
        );
    }
    // With text selected, "Copy" takes the selection, as the text view's own menu does; otherwise the message.
    let text = crate::feed_select::text().unwrap_or_else(|| plain_text(msg));
    if !text.is_empty() {
        items.push(
            widgets::menu_item(
                "menu-copy",
                Some(Icon::Copy),
                tr("ui.message_row.copy_text"),
                &theme,
            )
            .on_click(close_then(Box::new(move |_, cx| {
                cx.write_to_clipboard(ClipboardItem::new_string(text.clone()))
            })))
            .into_any_element(),
        );
    }
    if msg.editable && !msg.editing {
        let id = msg.id.clone();
        let delete_id = msg.id.clone();
        items.push(div().my_1().h(px(1.)).bg(theme.border).into_any_element());
        items.push(
            widgets::menu_item(
                "menu-edit",
                Some(Icon::Pencil),
                tr("ui.message_row.edit"),
                &theme,
            )
            .on_click(close_then(Box::new(move |this, _| {
                this.send(Action::Edit { id: id.clone() })
            })))
            .into_any_element(),
        );
        items.push(
            widgets::menu_item(
                "menu-delete",
                Some(Icon::Trash),
                tr("ui.message_row.delete_2"),
                &theme,
            )
            .text_color(theme.danger)
            .on_click(close_then(Box::new(move |this, _| {
                this.ui.conversation.confirm_delete = Some(delete_id.clone())
            })))
            .into_any_element(),
        );
    }
    items
}

fn content(
    this: &mut Oxide,
    msg: &MessageView,
    bubble: bool,
    window: &mut Window,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    let theme = this.theme;
    let caption = Caption {
        sender: if msg.mine {
            tr("ui.message_row.you")
        } else {
            msg.sender.clone()
        },
        time: msg.time.clone(),
    };
    let mut column = div()
        .min_w_0()
        .flex()
        .flex_col()
        .gap(px(8.))
        .when(msg.mine && bubble, |d| d.items_end())
        .when(!(msg.mine && bubble), |d| d.items_start());
    if msg.importance != Importance::Standard {
        column = column.child(importance_tag(msg.importance, this));
    }
    if let Some(subject) = &msg.subject {
        column = column.child(
            div()
                .text_size(px(16.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(subject.clone()),
        );
    }
    if msg.editing {
        // The composer's editor, drawn in the bubble being edited (Swift's `MessageEditor`).
        column = column.child(crate::composer::message_editor(this, &msg.id, window, cx));
    } else if msg.deleted {
        column = column.child(widgets::muted(
            tr("ui.message_row.this_message_has_been_deleted"),
            14.,
            &theme,
        ));
    } else {
        let size = if feed_blocks::emoji_only(&msg.blocks) {
            30.
        } else {
            14.
        };
        column = column.child(feed_blocks::body(
            this,
            &msg.id,
            &msg.blocks,
            size,
            Some(&caption),
            window,
            cx,
        ));
    }
    // Images the body does not place itself (file previews, images in tables).
    let inline: Vec<String> = feed_blocks::images(&msg.blocks)
        .into_iter()
        .map(|i| i.src.clone())
        .collect();
    for image in msg.images.iter().filter(|i| !inline.contains(&i.src)) {
        column = column.child(feed_blocks::preview_image(
            this,
            image,
            250.,
            true,
            Some(&caption),
            cx,
        ));
    }
    for file in &msg.files {
        column = column.child(
            div()
                .w(px(340f32.min(crate::feed::content_limit())))
                .child(crate::files::file_card(this, file, cx)),
        );
    }
    if let Some(link) = &msg.link_preview {
        column = column.child(
            div()
                .w(px(340f32.min(crate::feed::content_limit())))
                .child(feed_blocks::link_preview(this, link, &msg.id, cx)),
        );
    }
    column.into_any_element()
}

/// "IMPORTANT!" or "URGENT!" above a message, as Teams marks it.
pub fn importance_tag(importance: Importance, this: &Oxide) -> AnyElement {
    let theme = this.theme;
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
        .child(icon(glyph, px(11.), theme.error_text))
        .child(label)
        .into_any_element()
}

fn reaction_bar(this: &mut Oxide, msg: &MessageView) -> Option<AnyElement> {
    if msg.deleted || msg.reactions.is_empty() {
        return None;
    }
    let theme = this.theme;
    let mut bar = div().flex().flex_wrap().gap(px(6.));
    for reaction in &msg.reactions {
        let core = this.core.clone();
        let (id, key) = (msg.id.clone(), reaction.key.clone());
        let can = msg.can_react;
        bar = bar.child(
            div()
                .id(SharedString::from(format!(
                    "reaction-{}-{}",
                    msg.id, reaction.key
                )))
                .h(px(24.))
                .px(px(8.))
                .flex()
                .items_center()
                .gap(px(4.))
                .rounded_full()
                .border_1()
                .border_color(if reaction.mine {
                    theme.accent
                } else {
                    theme.border
                })
                .bg(if reaction.mine {
                    theme.accent_soft
                } else {
                    theme.surface
                })
                .when(can, |d| d.cursor_pointer())
                .child(div().text_size(px(14.)).child(reaction.emoji.clone()))
                .child(
                    div()
                        .text_size(px(12.))
                        .when(reaction.mine, |d| d.font_weight(FontWeight::SEMIBOLD))
                        .child(reaction.count.to_string()),
                )
                .tooltip(widgets::tooltip(reaction.hint.clone(), theme))
                .on_click(move |_, _, _| {
                    if can {
                        core.send(Action::React {
                            id: id.clone(),
                            key: key.clone(),
                        })
                    }
                }),
        );
    }
    Some(bar.into_any_element())
}

/// "Delete this message?" over the window.
pub fn delete_dialog(this: &mut Oxide, cx: &mut Context<Oxide>) -> Option<AnyElement> {
    let id = this.ui.conversation.confirm_delete.clone()?;
    let theme = this.theme;
    let cancel = cx.listener(|this, _: &gpui::ClickEvent, _, cx| {
        this.ui.conversation.confirm_delete = None;
        cx.notify();
    });
    let delete = cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
        this.ui.conversation.confirm_delete = None;
        this.send(Action::Delete { id: id.clone() });
        cx.notify();
    });
    let close = cx.listener(|this, _: &(), _, cx| {
        this.ui.conversation.confirm_delete = None;
        cx.notify();
    });
    Some(widgets::modal(
        "delete-message",
        px(360.),
        div()
            .p_5()
            .flex()
            .flex_col()
            .gap_3()
            .child(widgets::title(
                tr("ui.message_row.delete_this_message"),
                15.,
            ))
            .child(widgets::muted(
                tr("ui.message_row.it_will_be_replaced_with_this"),
                13.,
                &theme,
            ))
            .child(
                div()
                    .pt_2()
                    .flex()
                    .justify_end()
                    .gap_2()
                    .child(
                        feed::outline("delete-cancel", tr("ui.message_row.cancel"), this)
                            .on_click(cancel),
                    )
                    .child(
                        widgets::button(
                            "delete-confirm",
                            tr("ui.message_row.delete"),
                            ButtonKind::Danger,
                            &theme,
                        )
                        .rounded(px(8.))
                        .on_click(delete),
                    ),
            ),
        &theme,
        move |w, cx| close(&(), w, cx),
    ))
}

/// The right-click menu, at the click.
pub fn context_menu(this: &mut Oxide, cx: &mut Context<Oxide>) -> Option<AnyElement> {
    if !this.popup_open(MENU_KEY) {
        return None;
    }
    let menu = this.ui.conversation.menu.clone()?;
    let theme = this.theme;
    let items = menu_items(this, &menu.message, &menu.kind, cx);
    if items.is_empty() {
        return None;
    }
    let close = cx.listener(|this, _: &(), _, cx| this.close_popup(cx));
    Some(
        gpui::deferred(
            gpui::anchored()
                .position(menu.at)
                .snap_to_window_with_margin(px(8.))
                .child(
                    div()
                        .occlude()
                        .rounded(px(10.))
                        .border_1()
                        .border_color(theme.border)
                        .bg(theme.surface)
                        .shadow_lg()
                        .py_1()
                        .font_family(crate::oxide::FONT)
                        .text_color(theme.text)
                        .on_mouse_down_out(move |_, w, cx| close(&(), w, cx))
                        .children(items),
                ),
        )
        .with_priority(1)
        .into_any_element(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quote_line_cuts_long_messages() {
        let run = |text: &str| teams_core::RunView {
            text: text.into(),
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
        let mut msg = test_message();
        // The text comes from the blocks: the feed drops the flat runs it receives.
        let text = |t: &str| vec![teams_core::BlockView::Text { runs: vec![run(t)] }];
        msg.blocks = text("one\ntwo");
        assert!(quote_line(&msg).contains("one two"));
        msg.blocks = text(&"x".repeat(200));
        assert!(quote_line(&msg).ends_with('…'));
    }

    pub fn test_message() -> MessageView {
        MessageView {
            id: "m1".into(),
            sender: "Alice".into(),
            sender_id: "8:alice".into(),
            sender_avatar_shape: teams_core::AvatarShape::Round,
            sender_presence: None,
            mine: false,
            time: "10:00".into(),
            day: "Today".into(),
            subject: None,
            importance: Importance::Standard,
            deleted: false,
            editing: false,
            runs: vec![],
            blocks: vec![],
            images: vec![],
            files: vec![],
            link_preview: None,
            reactions: vec![],
            pending: false,
            can_react: true,
            editable: false,
            receipt: None,
        }
    }
}

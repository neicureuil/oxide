//! Group chat dialogs (`ChatAdmin.swift`): leaving a chat, and "Add people" with how much history they see.
//! The name / add / leave controls (`GroupChatControls`) are drawn by the members screen.
use crate::chats_parts::Echo;
use crate::icons::{Icon, icon};
use crate::input::{InputEvent, TextInput};
use crate::oxide::{Oxide, tr, trf};
use crate::widgets::{self, ButtonKind};
use gpui::{AnyElement, Context, Entity, FontWeight, SharedString, Window, div, prelude::*, px};
use teams_core::{Action, AddPeopleView, ChatAdminAction, ShareHistory};

pub struct Ui {
    /// The chat to confirm leaving: id and title.
    leaving: Option<(String, String)>,
    /// "Add people" search field.
    query: Entity<TextInput>,
    query_echo: Echo,
    /// Days of history for `ShareHistory::Days`, kept while the dialog is open.
    days: u32,
    /// The dialog was open at the last render: its fields start from the core's values when it opens.
    adding_open: bool,
}

impl Ui {
    pub fn new(window: &mut Window, cx: &mut Context<Oxide>) -> Self {
        let query = cx.new(|cx| TextInput::new(cx, tr("ui.chat_admin.enter_a_name_or_email")));
        cx.subscribe_in(
            &query,
            window,
            |this: &mut Oxide, input, event: &InputEvent, window, cx| match event {
                InputEvent::Changed => {
                    let text = input.read(cx).text().to_owned();
                    this.ui.chat_admin.query_echo.typed(&text);
                    this.send(admin(ChatAdminAction::AddQuery { query: text }));
                }
                InputEvent::Submit => this.send(admin(ChatAdminAction::AddSearch)),
                InputEvent::Cancel => {
                    this.send(admin(ChatAdminAction::CancelAdding));
                    window.blur();
                }
                _ => {}
            },
        )
        .detach();
        Self {
            leaving: None,
            query,
            query_echo: Echo::default(),
            days: 1,
            adding_open: false,
        }
    }
}

fn admin(action: ChatAdminAction) -> Action {
    Action::ChatAdmin { action }
}

/// Asks to confirm leaving a chat (from the chat list's menu, the conversation's header, the members screen).
pub fn ask_leave(this: &mut Oxide, chat_id: String, title: String, cx: &mut Context<Oxide>) {
    this.ui.chat_admin.leaving = Some((chat_id, title));
    cx.notify();
}

/// Dialogs and floating panels of this module, drawn over the window.
pub fn overlay(
    this: &mut Oxide,
    window: &mut Window,
    cx: &mut Context<Oxide>,
) -> Option<AnyElement> {
    let adding = this.chat_admin.as_ref().and_then(|a| a.adding.clone());
    let opened = adding.is_some() && !this.ui.chat_admin.adding_open;
    this.ui.chat_admin.adding_open = adding.is_some();
    if let Some((id, title)) = this.ui.chat_admin.leaving.clone() {
        return Some(leave_dialog(this, id, title, cx));
    }
    let adding = adding?;
    if opened {
        let ui = &mut this.ui.chat_admin;
        ui.query_echo.reset();
        ui.days = adding.days.max(1);
        ui.query.read(cx).focus(window);
    }
    let query = adding.query.clone();
    this.ui
        .chat_admin
        .query_echo
        .incoming(&query, &this.ui.chat_admin.query.clone(), cx);
    Some(add_people(this, &adding, cx))
}

/// The confirmation a menu cannot hold itself: leaving is undone only by someone adding you again.
fn leave_dialog(
    this: &mut Oxide,
    id: String,
    title: String,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    let theme = this.theme;
    let entity = cx.entity().downgrade();
    let close = move |_: &mut Window, cx: &mut gpui::App| {
        let _ = entity.update(cx, |this, cx| {
            this.ui.chat_admin.leaving = None;
            cx.notify();
        });
    };
    let content = div()
        .p(px(20.))
        .flex()
        .flex_col()
        .gap_3()
        .child(widgets::title(
            trf("ui.chat_admin.leave_question", &[("title", &title)]),
            15.,
        ))
        .child(widgets::muted(
            tr("ui.chat_admin.leave_explanation"),
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
                    widgets::button(
                        "leave-cancel",
                        tr("ui.chat_admin.cancel"),
                        ButtonKind::Outline,
                        &theme,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.ui.chat_admin.leaving = None;
                        cx.notify();
                    })),
                )
                .child(
                    widgets::button(
                        "leave-confirm",
                        tr("ui.chat_admin.leave"),
                        ButtonKind::Danger,
                        &theme,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.send(admin(ChatAdminAction::Leave {
                            chat_id: id.clone(),
                        }));
                        this.ui.chat_admin.leaving = None;
                        cx.notify();
                    })),
                ),
        );
    widgets::modal("leave-dialog", px(380.), content, &theme, close)
}

/// "Add people": search the directory, pick people, choose how much history they see (`AddPeopleSheet`).
fn add_people(this: &mut Oxide, adding: &AddPeopleView, cx: &mut Context<Oxide>) -> AnyElement {
    let theme = this.theme;
    let days = this.ui.chat_admin.days;
    let short = this
        .ui
        .chat_admin
        .query
        .read(cx)
        .text()
        .trim()
        .chars()
        .count()
        < 2;
    let search_disabled = adding.searching || short;

    let mut body = div()
        .p(px(20.))
        .flex()
        .flex_col()
        .gap(px(14.))
        .child(widgets::title(tr("ui.chat_admin.add_people"), 17.));

    if !adding.chosen.is_empty() {
        let mut chips = div()
            .id("chosen-people")
            .flex()
            .gap(px(6.))
            .overflow_x_scroll();
        for person in &adding.chosen {
            let mri = person.mri.clone();
            chips = chips.child(
                div()
                    .flex_none()
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .py_1()
                    .rounded_full()
                    .bg(theme.accent_soft)
                    .text_size(px(12.))
                    .child(person.name.clone())
                    .child(
                        div()
                            .id(SharedString::from(format!("unchoose-{}", person.mri)))
                            .cursor_pointer()
                            .child(icon(Icon::Close, px(9.), theme.text))
                            .tooltip(widgets::tooltip(
                                trf("ui.chat_admin.remove_person", &[("name", &person.name)]),
                                theme,
                            ))
                            .on_click(cx.listener(move |this, _, _, _| {
                                this.send(admin(ChatAdminAction::Unchoose { mri: mri.clone() }))
                            })),
                    ),
            );
        }
        body = body.child(chips);
    }

    body = body.child(
        div()
            .flex()
            .items_center()
            .gap_2()
            .child(crate::chats_parts::row_field(
                &this.ui.chat_admin.query,
                &theme,
                cx,
            ))
            .child(
                widgets::disabled(
                    widgets::button(
                        "add-search",
                        tr(if adding.searching {
                            "ui.chat_admin.searching"
                        } else {
                            "ui.chat_admin.search"
                        }),
                        ButtonKind::Outline,
                        &theme,
                    ),
                    search_disabled,
                )
                .on_click(cx.listener(move |this, _, _, _| {
                    if !search_disabled {
                        this.send(admin(ChatAdminAction::AddSearch));
                    }
                })),
            ),
    );

    if !adding.results.is_empty() {
        let mut results = div()
            .id("add-results")
            .max_h(px(200.))
            .overflow_y_scroll()
            .flex()
            .flex_col()
            .gap(px(2.));
        for person in &adding.results {
            let avatar = this.avatar(
                &person.mri,
                &person.name,
                28.,
                teams_core::AvatarShape::Round,
                None,
            );
            let mri = person.mri.clone();
            let hover = theme.input;
            results = results.child(
                div()
                    .id(SharedString::from(format!("add-{}", person.mri)))
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .p(px(6.))
                    .rounded(px(8.))
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover))
                    .child(avatar)
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(1.))
                            .child(div().text_size(px(13.)).child(person.name.clone()))
                            .when(!person.email.is_empty(), |d| {
                                d.child(widgets::muted(person.email.clone(), 11., &theme))
                            }),
                    )
                    .on_click(cx.listener(move |this, _, _, _| {
                        this.send(admin(ChatAdminAction::Choose { mri: mri.clone() }))
                    })),
            );
        }
        body = body.child(results);
    } else if adding.searched {
        body = body.child(widgets::muted(
            tr("ui.chat_admin.nobody_found"),
            12.,
            &theme,
        ));
    }

    body = body.child(widgets::divider(&theme));
    let mut history = div().flex().flex_col().gap_2().text_size(px(13.)).child(
        div()
            .font_weight(FontWeight::MEDIUM)
            .child(tr("ui.chat_admin.history")),
    );
    for (choice, key) in [
        (ShareHistory::None, "ui.chat_admin.no_history"),
        (ShareHistory::Days, "ui.chat_admin.history_days"),
        (ShareHistory::All, "ui.chat_admin.all_history"),
    ] {
        history = history.child(
            widgets::radio(
                SharedString::from(format!("history-{key}")),
                adding.history == choice,
                tr(key),
                &theme,
            )
            .on_click(cx.listener(move |this, _, _, _| {
                let days = this.ui.chat_admin.days;
                this.send(admin(ChatAdminAction::SetHistory {
                    history: choice,
                    days,
                }));
            })),
        );
    }
    body = body.child(history);
    if adding.history == ShareHistory::Days {
        let step = |id: &'static str, glyph: Icon, delta: i32, cx: &mut Context<Oxide>| {
            let next = (days as i32 + delta).clamp(1, 365) as u32;
            widgets::icon_button_sized(id, glyph, 24., 12., false, &theme).on_click(cx.listener(
                move |this, _, _, cx| {
                    this.ui.chat_admin.days = next;
                    this.send(admin(ChatAdminAction::SetHistory {
                        history: ShareHistory::Days,
                        days: next,
                    }));
                    cx.notify();
                },
            ))
        };
        body = body.child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .text_size(px(13.))
                .child(trf(
                    "ui.chat_admin.days_count",
                    &[("count", &days.to_string())],
                ))
                .child(
                    div()
                        .flex()
                        .rounded(px(6.))
                        .border_1()
                        .border_color(theme.border_strong)
                        .child(step("days-less", Icon::ChevronDown, -1, cx))
                        .child(step("days-more", Icon::ChevronUp, 1, cx)),
                ),
        );
    }

    let add_disabled = adding.chosen.is_empty() || adding.adding;
    body = body.child(
        div()
            .flex()
            .justify_end()
            .gap_2()
            .child(
                widgets::button(
                    "add-cancel",
                    tr("ui.chat_admin.cancel"),
                    ButtonKind::Outline,
                    &theme,
                )
                .on_click(
                    cx.listener(|this, _, _, _| this.send(admin(ChatAdminAction::CancelAdding))),
                ),
            )
            .child(
                widgets::disabled(
                    widgets::button(
                        "add-confirm",
                        tr(if adding.adding {
                            "ui.chat_admin.adding"
                        } else {
                            "ui.chat_admin.add"
                        }),
                        ButtonKind::Primary,
                        &theme,
                    ),
                    add_disabled,
                )
                .on_click(cx.listener(move |this, _, _, _| {
                    if !add_disabled {
                        this.send(admin(ChatAdminAction::ConfirmAdd));
                    }
                })),
            ),
    );

    let core = this.core.clone();
    widgets::modal("add-people", px(460.), body, &theme, move |_, _| {
        core.send(admin(ChatAdminAction::CancelAdding))
    })
}

/// A step of `OXIDE_SCRIPT` this module understands: `admin:add` (opens "Add people" for the open chat),
/// `admin:search <text>`, `admin:leave <n>` (asks to leave the n-th chat of the list).
pub fn debug_step(
    this: &mut Oxide,
    step: &str,
    window: &mut Window,
    cx: &mut Context<Oxide>,
) -> bool {
    let Some(rest) = step.strip_prefix("admin:") else {
        return false;
    };
    let (verb, arg) = rest.split_once(' ').unwrap_or((rest, ""));
    match verb {
        "add" => this.send(admin(ChatAdminAction::StartAdding)),
        "search" => {
            let text = arg.to_owned();
            this.ui
                .chat_admin
                .query
                .update(cx, |i, cx| i.set_text(text.clone(), None, cx));
            this.ui.chat_admin.query_echo.typed(&text);
            this.send(admin(ChatAdminAction::AddQuery { query: text }));
            this.send(admin(ChatAdminAction::AddSearch));
        }
        "leave" => {
            let Some(chat) = arg
                .parse::<usize>()
                .ok()
                .and_then(|n| this.sidebar.as_ref()?.chats.get(n).cloned())
            else {
                return false;
            };
            ask_leave(this, chat.id, chat.title, cx);
        }
        _ => return false,
    }
    let _ = window;
    true
}

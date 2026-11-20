//! The new chat screen (`NewChatScreen` in `Screens.swift`): find people, pick them, name a group, create.
use crate::chats_parts::Echo;
use crate::icons::Icon;
use crate::input::{InputEvent, TextInput};
use crate::oxide::{Oxide, tr};
use crate::widgets::{self, ButtonKind};
use gpui::{AnyElement, Context, Entity, FontWeight, SharedString, Window, div, prelude::*, px};
use teams_core::{Action, AvatarShape, NewChatView};

pub struct Ui {
    query: Entity<TextInput>,
    query_echo: Echo,
    title: Entity<TextInput>,
    title_echo: Echo,
    /// The screen was open at the last render: it starts afresh, cursor in the search, when it opens.
    open: bool,
}

impl Ui {
    pub fn new(_window: &mut Window, cx: &mut Context<Oxide>) -> Self {
        let query = cx.new(|cx| TextInput::new(cx, tr("ui.screens.name_or_email_address")));
        cx.subscribe(
            &query,
            |this: &mut Oxide, input, event: &InputEvent, cx| match event {
                InputEvent::Changed => {
                    let text = input.read(cx).text().to_owned();
                    this.ui.new_chat.query_echo.typed(&text);
                    this.send(Action::PeopleQuery { query: text });
                }
                InputEvent::Submit => this.send(Action::PeopleSearch),
                InputEvent::Cancel => this.send(Action::CancelNewChat),
                _ => {}
            },
        )
        .detach();
        let title = cx.new(|cx| TextInput::new(cx, tr("ui.screens.group_name_optional")));
        cx.subscribe(
            &title,
            |this: &mut Oxide, input, event: &InputEvent, cx| match event {
                InputEvent::Changed => {
                    let text = input.read(cx).text().to_owned();
                    this.ui.new_chat.title_echo.typed(&text);
                    this.send(Action::NewTitle { title: text });
                }
                InputEvent::Cancel => this.send(Action::CancelNewChat),
                _ => {}
            },
        )
        .detach();
        Self {
            query,
            query_echo: Echo::default(),
            title,
            title_echo: Echo::default(),
            open: false,
        }
    }
}

pub fn screen(this: &mut Oxide, window: &mut Window, cx: &mut Context<Oxide>) -> AnyElement {
    let Some(dialog) = this.new_chat.clone() else {
        this.ui.new_chat.open = false;
        return div().into_any_element();
    };
    let theme = this.theme;
    {
        let ui = &mut this.ui.new_chat;
        if !ui.open {
            ui.open = true;
            ui.query_echo.reset();
            ui.title_echo.reset();
            ui.query.read(cx).focus(window);
        }
        let (query, title) = (ui.query.clone(), ui.title.clone());
        ui.query_echo.incoming(&dialog.query, &query, cx);
        ui.title_echo.incoming(&dialog.title, &title, cx);
    }

    let short = dialog.query.trim().chars().count() < 2;
    let search_disabled = dialog.searching || short;
    let mut card = widgets::card(&theme)
        .rounded(px(16.))
        .w_full()
        .max_w(px(650.))
        .p(px(28.))
        .flex()
        .flex_col()
        .gap(px(18.))
        .child(
            div()
                .flex()
                .items_center()
                .child(
                    div()
                        .flex_1()
                        .child(widgets::title(tr("ui.screens.new_chat"), 22.)),
                )
                .child(
                    widgets::icon_button_sized(
                        "new-chat-close",
                        Icon::Close,
                        34.,
                        16.,
                        false,
                        &theme,
                    )
                    .tooltip(widgets::tooltip(tr("ui.screens.cancel"), theme))
                    .on_click(cx.listener(|this, _, _, _| this.send(Action::CancelNewChat))),
                ),
        )
        .child(widgets::muted(
            tr("ui.screens.search_for_a_person_or_pick"),
            13.,
            &theme,
        ))
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(crate::chats_parts::row_field(
                    &this.ui.new_chat.query,
                    &theme,
                    cx,
                ))
                .child(
                    widgets::disabled(
                        widgets::button(
                            "people-search",
                            tr(if dialog.searching {
                                "ui.screens.searching"
                            } else {
                                "ui.screens.search"
                            }),
                            ButtonKind::Outline,
                            &theme,
                        ),
                        search_disabled,
                    )
                    .on_click(cx.listener(move |this, _, _, _| {
                        if !search_disabled {
                            this.send(Action::PeopleSearch);
                        }
                    })),
                ),
        );

    if !dialog.chosen.is_empty() {
        card = card.child(chosen(this, &dialog, cx));
    }
    card = card.child(results(this, &dialog, cx));
    if dialog.chosen.len() > 1 {
        card = card.child(crate::widgets::text_field(
            &this.ui.new_chat.title,
            &theme,
            cx,
        ));
    }
    let create_disabled = dialog.chosen.is_empty() || dialog.creating;
    card = card.child(
        div()
            .flex()
            .items_center()
            .child(
                widgets::button(
                    "new-chat-cancel",
                    tr("ui.screens.cancel"),
                    ButtonKind::Outline,
                    &theme,
                )
                .on_click(cx.listener(|this, _, _, _| this.send(Action::CancelNewChat))),
            )
            .child(div().flex_1())
            .child(
                widgets::disabled(
                    widgets::button(
                        "new-chat-create",
                        tr(if dialog.creating {
                            "ui.screens.creating"
                        } else {
                            "ui.screens.create_chat"
                        }),
                        ButtonKind::Primary,
                        &theme,
                    ),
                    create_disabled,
                )
                .on_click(cx.listener(move |this, _, _, _| {
                    if !create_disabled {
                        this.send(Action::CreateChat);
                    }
                })),
            ),
    );

    div()
        .id("new-chat")
        .size_full()
        .overflow_y_scroll()
        .child(
            div()
                .w_full()
                .p(px(28.))
                .flex()
                .justify_center()
                .child(card),
        )
        .into_any_element()
}

/// The people picked so far, each with its remove button.
fn chosen(this: &mut Oxide, dialog: &NewChatView, cx: &mut Context<Oxide>) -> AnyElement {
    let theme = this.theme;
    let mut column =
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(widgets::muted(tr("ui.screens.to"), 11., &theme));
    for person in &dialog.chosen {
        let avatar = this.avatar(&person.mri, &person.name, 30., AvatarShape::Round, None);
        let mri = person.mri.clone();
        column = column.child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(avatar)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .line_clamp(1)
                        .text_ellipsis()
                        .child(person.name.clone()),
                )
                .child(
                    widgets::icon_button_sized(
                        SharedString::from(format!("remove-{}", person.mri)),
                        Icon::Close,
                        34.,
                        16.,
                        false,
                        &theme,
                    )
                    .tooltip(widgets::tooltip(tr("ui.screens.remove"), theme))
                    .on_click(cx.listener(move |this, _, _, _| {
                        this.send(Action::RemovePerson { mri: mri.clone() })
                    })),
                ),
        );
    }
    column.into_any_element()
}

fn results(this: &mut Oxide, dialog: &NewChatView, cx: &mut Context<Oxide>) -> AnyElement {
    let theme = this.theme;
    let mut column = div().flex().flex_col().gap_1();
    if dialog.searched && dialog.results.is_empty() && !dialog.query.is_empty() {
        column = column.child(widgets::muted(
            tr("ui.screens.no_people_found_for_this_search"),
            13.,
            &theme,
        ));
    }
    for person in &dialog.results {
        let avatar = this.avatar(&person.mri, &person.name, 34., AvatarShape::Round, None);
        let mri = person.mri.clone();
        let hover = theme.input;
        column = column.child(
            div()
                .id(SharedString::from(format!("person-{}", person.mri)))
                .flex()
                .items_center()
                .gap(px(10.))
                .p(px(10.))
                .rounded(px(8.))
                .cursor_pointer()
                .hover(move |s| s.bg(hover))
                .child(avatar)
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(3.))
                        .child(
                            div()
                                .text_size(px(13.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme.text)
                                .child(person.name.clone()),
                        )
                        .when(!person.email.is_empty(), |d| {
                            d.child(widgets::muted(person.email.clone(), 11., &theme))
                        }),
                )
                .on_click(cx.listener(move |this, _, _, _| {
                    this.send(Action::ChoosePerson { mri: mri.clone() })
                })),
        );
    }
    column.into_any_element()
}

/// Dialogs and floating panels of this module, drawn over the window.
pub fn overlay(
    _this: &mut Oxide,
    _window: &mut Window,
    _cx: &mut Context<Oxide>,
) -> Option<AnyElement> {
    None
}

/// A step of `OXIDE_SCRIPT` this module understands: `newchat:query <text>` (types and searches),
/// `newchat:choose <n>` (n-th result), `newchat:title <text>`.
pub fn debug_step(
    this: &mut Oxide,
    step: &str,
    _window: &mut Window,
    cx: &mut Context<Oxide>,
) -> bool {
    let Some(rest) = step.strip_prefix("newchat:") else {
        return false;
    };
    let (verb, arg) = rest.split_once(' ').unwrap_or((rest, ""));
    match verb {
        "query" => {
            let text = arg.to_owned();
            this.ui
                .new_chat
                .query
                .update(cx, |i, cx| i.set_text(text.clone(), None, cx));
            this.ui.new_chat.query_echo.typed(&text);
            this.send(Action::PeopleQuery { query: text });
            this.send(Action::PeopleSearch);
        }
        "choose" => {
            let Some(person) = arg
                .parse::<usize>()
                .ok()
                .and_then(|n| this.new_chat.as_ref()?.results.get(n).cloned())
            else {
                return false;
            };
            this.send(Action::ChoosePerson { mri: person.mri });
        }
        "title" => {
            let text = arg.to_owned();
            this.ui
                .new_chat
                .title
                .update(cx, |i, cx| i.set_text(text.clone(), None, cx));
            this.ui.new_chat.title_echo.typed(&text);
            this.send(Action::NewTitle { title: text });
        }
        _ => return false,
    }
    true
}

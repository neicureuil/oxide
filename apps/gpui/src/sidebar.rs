//! The chat list column (`SidebarView.swift`): title and buttons, filter chips, favourites and recent chats,
//! each chat's context menu. The activity column reuses its header and chips.
use crate::chat_admin;
use crate::chats_parts::{SyncedList, context_menu, menu_separator, sig};
use crate::icons::{Icon, icon};
use crate::oxide::{Oxide, tr};
use crate::theme::Theme;
use crate::widgets;
use gpui::{
    AnyElement, Context, FocusHandle, FontWeight, KeyDownEvent, MouseButton, MouseDownEvent,
    Pixels, Point, SharedString, Window, div, list, prelude::*, px,
};
use teams_core::{
    Action, ChatAdminAction, ChatRow, Filter, Nav, NotificationsAction, Shell, Target,
};

pub struct Ui {
    list: SyncedList,
    /// What the list shows, row by row, rebuilt at each render.
    items: Vec<Item>,
    /// ↑ and ↓ switch chats while the list has the focus, which a click on a chat gives it.
    focus: FocusHandle,
    /// The chat whose context menu is open, and where it was asked.
    menu: Option<(String, Point<Pixels>)>,
}

impl Ui {
    pub fn new(_window: &mut Window, cx: &mut Context<Oxide>) -> Self {
        Self {
            list: SyncedList::new(),
            items: Vec::new(),
            focus: cx.focus_handle(),
            menu: None,
        }
    }
}

#[derive(Clone, Copy, Hash)]
enum Item {
    /// Favourites (true) or recent chats, with the room above it.
    Header(bool, u8),
    /// Index in `Sidebar::chats`.
    Chat(usize),
    More,
}

const MENU: &str = "chat-menu";

pub fn chats(this: &mut Oxide, cx: &mut Context<Oxide>) -> AnyElement {
    let theme = this.theme;
    let Some(shell) = this.shell.clone() else {
        return div().into_any_element();
    };
    let column = div()
        .size_full()
        .flex()
        .flex_col()
        .bg(theme.surface)
        .child(header(this, &shell, cx));
    if this.sidebar.is_none() {
        return column.into_any_element();
    }
    column
        .child(chips(this, &shell))
        .child(chat_list(this, &shell, cx))
        .into_any_element()
}

/// Title and buttons of the chat and activity columns.
pub fn header(this: &mut Oxide, shell: &Shell, cx: &mut Context<Oxide>) -> AnyElement {
    let theme = this.theme;
    let activity = shell.nav == Nav::Activity;
    let title = if activity {
        tr("ui.sidebar_view.activity")
    } else {
        tr("ui.sidebar_view.chat")
    };
    let filter = shell.filter;
    let mut row = div()
        .flex()
        .items_center()
        .gap_1()
        .pt(px(16.))
        .pl(px(16.))
        .pb(px(10.))
        .pr(px(12.))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(px(20.))
                .line_height(px(26.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(title),
        )
        .child(
            widgets::icon_button_sized("column-filter", Icon::Filter, 34., 16., false, &theme)
                .tooltip(widgets::tooltip(tr("ui.sidebar_view.unread"), theme))
                .on_click(cx.listener(move |this, _, _, _| {
                    let filter = if filter == Filter::Unread {
                        Filter::All
                    } else {
                        Filter::Unread
                    };
                    this.send(Action::SetFilter { filter });
                })),
        );
    if activity {
        let none_unread = shell.activity_unread == 0;
        let loading = this.sidebar.as_ref().is_some_and(|s| s.activity_loading);
        row = row
            .child(
                widgets::disabled(
                    widgets::icon_button_sized(
                        "mark-all-read",
                        Icon::Check,
                        34.,
                        16.,
                        false,
                        &theme,
                    ),
                    none_unread,
                )
                .tooltip(widgets::tooltip(
                    tr("ui.sidebar_view.mark_all_as_read"),
                    theme,
                ))
                .on_click(cx.listener(move |this, _, _, _| {
                    if !none_unread {
                        this.send(Action::Notifications {
                            action: NotificationsAction::MarkAllActivityRead,
                        });
                    }
                })),
            )
            .child(refresh_button(
                "activity-refresh",
                tr("ui.sidebar_view.refresh_notifications"),
                loading,
                &theme,
                cx,
            ));
    } else {
        let hover = theme.input;
        row = row.child(
            div()
                .id("new-chat")
                .size(px(34.))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(8.))
                .cursor_pointer()
                .hover(move |s| s.bg(hover))
                .child(icon(Icon::Compose, px(16.), theme.accent_text))
                .tooltip(widgets::tooltip(tr("ui.sidebar_view.new_chat"), theme))
                .on_click(cx.listener(|this, _, _, _| this.send(Action::NewChat))),
        );
    }
    row.into_any_element()
}

/// An arrow that turns while a refresh is running.
pub fn refresh_button(
    id: &'static str,
    help: String,
    spinning: bool,
    theme: &Theme,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    let theme = *theme;
    if spinning {
        return div()
            .size(px(34.))
            .flex_none()
            .flex()
            .items_center()
            .justify_center()
            .child(widgets::spinner(
                SharedString::from(format!("{id}-spin")),
                16.,
                theme.muted,
            ))
            .into_any_element();
    }
    widgets::icon_button_sized(id, Icon::Refresh, 34., 16., false, &theme)
        .tooltip(widgets::tooltip(help, theme))
        .on_click(cx.listener(|this, _, _, _| this.send(Action::Refresh)))
        .into_any_element()
}

/// All / Unread · n / Meetings (not on the activity column).
pub fn chips(this: &mut Oxide, shell: &Shell) -> AnyElement {
    let theme = this.theme;
    let activity = shell.nav == Nav::Activity;
    let unread = if activity {
        shell.activity_unread
    } else {
        shell.unread
    };
    let mut items = vec![
        (tr("ui.sidebar_view.all"), Filter::All),
        (
            format!("{} · {unread}", tr("ui.sidebar_view.unread")),
            Filter::Unread,
        ),
    ];
    if !activity {
        items.push((tr("ui.sidebar_view.meetings"), Filter::Meetings));
    }
    let selected = items.iter().position(|(_, f)| *f == shell.filter);
    let key = if activity { "activity-chips" } else { "chat-chips" };
    // The selected chip's capsule slides to the new one (`matchedGeometryEffect`, select spring).
    let (highlight, slid, measure) = crate::motion::sliding_selection(key, selected, 16., move |d| {
        d.top_0().h(px(28.)).rounded_full().bg(theme.accent_soft)
    });
    let mut row = div()
        .relative()
        .flex()
        .items_center()
        .gap(px(6.))
        .pl(px(16.))
        .pr(px(12.))
        .pb(px(12.))
        .on_children_prepainted(measure)
        .child(highlight.unwrap_or_else(|| div().into_any_element()));
    for (label, filter) in items {
        let active = shell.filter == filter;
        let core = this.core.clone();
        row = row.child(
            div()
                .id(SharedString::from(format!("chip-{filter:?}")))
                .h(px(28.))
                .px(px(12.))
                .flex()
                .items_center()
                .rounded_full()
                .cursor_pointer()
                .text_size(px(12.))
                .when(active, |d| {
                    d.when(!slid, |d| d.bg(theme.accent_soft))
                        .text_color(theme.accent_text)
                        .font_weight(FontWeight::SEMIBOLD)
                })
                .when(!active, |d| d.bg(theme.input).text_color(theme.text_soft))
                .child(label)
                .on_click(move |_, _, _| core.send(Action::SetFilter { filter })),
        );
    }
    row.into_any_element()
}

fn chat_list(this: &mut Oxide, shell: &Shell, cx: &mut Context<Oxide>) -> AnyElement {
    let theme = this.theme;
    let Some(sidebar) = this.sidebar.as_ref() else {
        return div().into_any_element();
    };
    let loading = shell.loading_workspace;
    if sidebar.chats.is_empty() {
        this.ui.sidebar.items.clear();
        this.ui.sidebar.list.clear();
        let body = if loading {
            skeleton(8, 40., &theme).pt_2().into_any_element()
        } else {
            div()
                .p(px(20.))
                .child(widgets::muted(
                    tr("ui.sidebar_view.no_chats_to_show"),
                    12.,
                    &theme,
                ))
                .into_any_element()
        };
        return div()
            .flex_1()
            .px_2()
            .child(body)
            .when(sidebar.more_chats, |d| {
                d.child(more_button(this, loading, cx))
            })
            .into_any_element();
    }

    // Favourites first, as the web client.
    let pinned: Vec<usize> = (0..sidebar.chats.len())
        .filter(|&i| sidebar.chats[i].pinned)
        .collect();
    let recent: Vec<usize> = (0..sidebar.chats.len())
        .filter(|&i| !sidebar.chats[i].pinned)
        .collect();
    let mut items = Vec::with_capacity(sidebar.chats.len() + 3);
    if !pinned.is_empty() {
        items.push(Item::Header(true, 8));
        items.extend(pinned.into_iter().map(Item::Chat));
    }
    if !recent.is_empty() {
        let top = if items.is_empty() { 8 } else { 14 };
        items.push(Item::Header(false, top));
        items.extend(recent.into_iter().map(Item::Chat));
    }
    if sidebar.more_chats {
        items.push(Item::More);
    }
    let sigs = items
        .iter()
        .map(|item| match item {
            Item::Chat(i) => sig(&sidebar.chats[*i]),
            other => sig(other),
        })
        .collect();
    this.ui.sidebar.items = items;
    this.ui.sidebar.list.sync(sigs);

    let entity = cx.entity();
    let focus = this.ui.sidebar.focus.clone();
    div()
        .id("chat-list")
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .track_focus(&focus)
        .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
            let offset = match event.keystroke.key.as_str() {
                "up" => -1,
                "down" => 1,
                _ => return,
            };
            move_selection(this, offset);
            cx.stop_propagation();
        }))
        .child(
            list(this.ui.sidebar.list.state.clone(), move |ix, window, cx| {
                entity.update(cx, |this, cx| item(this, ix, window, cx))
            })
            .flex_1()
            .pb(px(12.)),
        )
        .into_any_element()
}

fn item(this: &mut Oxide, ix: usize, window: &mut Window, cx: &mut Context<Oxide>) -> AnyElement {
    let theme = this.theme;
    match this.ui.sidebar.items.get(ix).copied() {
        Some(Item::Header(favourites, top)) => div()
            .px_2()
            .child(
                div()
                    .pt(px(top as f32))
                    .px_2()
                    .pb(px(6.))
                    .text_size(px(11.))
                    .line_height(px(14.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_color(theme.subtle)
                    .child(if favourites {
                        tr("ui.sidebar_view.favorites")
                    } else {
                        tr("ui.sidebar_view.recent")
                    }),
            )
            .into_any_element(),
        Some(Item::Chat(index)) => {
            let Some(row) = this
                .sidebar
                .as_ref()
                .and_then(|s| s.chats.get(index))
                .cloned()
            else {
                return div().into_any_element();
            };
            let focused = this.ui.sidebar.focus.is_focused(window);
            div()
                .px_2()
                .child(chat_row(this, &row, focused, cx))
                .into_any_element()
        }
        Some(Item::More) => {
            let loading = this.shell.as_ref().is_some_and(|s| s.loading_workspace);
            div()
                .px_2()
                .child(more_button(this, loading, cx))
                .into_any_element()
        }
        None => div().into_any_element(),
    }
}

fn more_button(this: &mut Oxide, loading: bool, cx: &mut Context<Oxide>) -> AnyElement {
    let theme = this.theme;
    let hover = theme.input;
    div()
        .p_2()
        .flex()
        .child(
            div()
                .id("more-chats")
                .px_2()
                .py_1()
                .rounded(px(8.))
                .text_size(px(12.))
                .text_color(theme.muted)
                .when(!loading, |d| d.cursor_pointer().hover(move |s| s.bg(hover)))
                .when(loading, |d| d.opacity(0.45))
                .child(tr("ui.sidebar_view.more_chats"))
                .on_click(cx.listener(move |this, _, _, _| {
                    if !loading {
                        this.send(Action::MoreChats);
                    }
                })),
        )
        .into_any_element()
}

fn chat_row(this: &mut Oxide, row: &ChatRow, focused: bool, cx: &mut Context<Oxide>) -> AnyElement {
    let theme = this.theme;
    let avatar = this.avatar(
        &row.id,
        &row.title,
        40.,
        row.avatar_shape,
        row.presence.as_ref(),
    );
    let hover = theme.input;
    let id = row.id.clone();
    let menu_id = row.id.clone();
    let selected = row.selected;
    div()
        .id(SharedString::from(format!("chat-{}", row.id)))
        .flex()
        .items_center()
        .gap(px(12.))
        .px(px(10.))
        .py(px(8.))
        .rounded(px(8.))
        .border(px(1.5))
        .border_color(if selected && focused {
            theme.accent
        } else {
            gpui::transparent_black()
        })
        .cursor_pointer()
        .when(selected, |d| d.bg(theme.accent_soft))
        .map(|d| if selected { d } else { crate::motion::hover_fill(d, format!("chat-row-{id}"), hover, px(8.)) })
        .child(avatar)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .line_clamp(1)
                                .text_ellipsis()
                                .text_size(px(14.))
                                .line_height(px(18.))
                                .font_weight(if row.unread {
                                    FontWeight::SEMIBOLD
                                } else {
                                    FontWeight::MEDIUM
                                })
                                .child(row.title.clone()),
                        )
                        .when(row.hidden, |d| {
                            d.child(icon(Icon::Hide, px(11.), theme.subtle))
                        })
                        .when(row.muted, |d| {
                            d.child(icon(Icon::BellOff, px(11.), theme.subtle))
                        })
                        .child(
                            div()
                                .flex_none()
                                .text_size(px(12.))
                                .text_color(theme.subtle)
                                .child(row.time.clone()),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .line_clamp(1)
                                .text_ellipsis()
                                .text_size(px(13.))
                                .line_height(px(17.))
                                .text_color(if row.unread { theme.text } else { theme.muted })
                                .child(row.preview.clone()),
                        )
                        .when(row.unread, |d| {
                            let dot = div().flex_none().size(px(10.)).rounded_full().bg(theme.accent);
                            // Pops in when the chat turns unread (`transition(.pop)`), not on scroll.
                            if crate::motion::just_changed(&format!("unread-{}", row.id), 1.) {
                                d.child(crate::motion::pop(dot, SharedString::from(format!("unread-dot-{}", row.id))))
                            } else {
                                d.child(dot)
                            }
                        })
                        .when(!row.unread, |d| {
                            crate::motion::just_changed(&format!("unread-{}", row.id), 0.);
                            d
                        }),
                ),
        )
        .tooltip(widgets::tooltip(row.title.clone(), theme))
        .on_click(cx.listener(move |this, _, window, _| {
            window.focus(&this.ui.sidebar.focus);
            this.send(Action::Select {
                target: Target {
                    team: None,
                    id: id.clone(),
                },
            });
        }))
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                open_menu(this, menu_id.clone(), event.position, cx);
                cx.stop_propagation();
            }),
        )
        .into_any_element()
}

fn open_menu(this: &mut Oxide, id: String, position: Point<Pixels>, cx: &mut Context<Oxide>) {
    this.ui.sidebar.menu = Some((id, position));
    this.popup = Some(MENU.into());
    cx.notify();
}

/// What can be done to a chat, from its row in the list; the conversation's header menu can reuse
/// these items. Each closes the open popup once chosen.
pub fn chat_actions(this: &mut Oxide, row: &ChatRow, cx: &mut Context<Oxide>) -> Vec<AnyElement> {
    let theme = this.theme;
    let id = row.id.clone();
    let mut items = Vec::new();
    let entry =
        |key: &'static str, glyph: Icon, label: String, action: Action, cx: &mut Context<Oxide>| {
            widgets::menu_item(key, Some(glyph), label, &theme)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.send(action.clone());
                    this.close_popup(cx);
                }))
                .into_any_element()
        };
    items.push(entry(
        "chat-pin",
        Icon::Pin,
        tr(if row.pinned {
            "ui.chat_admin.unpin"
        } else {
            "ui.chat_admin.pin"
        }),
        Action::ChatAdmin {
            action: ChatAdminAction::SetPinned {
                chat_id: id.clone(),
                pinned: !row.pinned,
            },
        },
        cx,
    ));
    items.push(entry(
        "chat-read",
        Icon::CheckDouble,
        tr(if row.unread {
            "ui.chat_admin.mark_as_read"
        } else {
            "ui.chat_admin.mark_as_unread"
        }),
        Action::ChatAdmin {
            action: if row.unread {
                ChatAdminAction::MarkRead {
                    chat_id: id.clone(),
                }
            } else {
                ChatAdminAction::MarkUnread {
                    chat_id: id.clone(),
                }
            },
        },
        cx,
    ));
    items.push(entry(
        "chat-hide",
        if row.hidden { Icon::Eye } else { Icon::Hide },
        tr(if row.hidden {
            "ui.chat_admin.show"
        } else {
            "ui.chat_admin.hide"
        }),
        Action::ChatAdmin {
            action: if row.hidden {
                ChatAdminAction::Unhide {
                    chat_id: id.clone(),
                }
            } else {
                ChatAdminAction::Hide {
                    chat_id: id.clone(),
                }
            },
        },
        cx,
    ));
    items.push(entry(
        "chat-mute",
        if row.muted { Icon::Bell } else { Icon::BellOff },
        tr(if row.muted {
            "ui.notification_settings_view.unmute_notifications"
        } else {
            "ui.notification_settings_view.mute_notifications"
        }),
        Action::Notifications {
            action: NotificationsAction::SetChatMuted {
                conversation_id: id.clone(),
                muted: !row.muted,
            },
        },
        cx,
    ));
    items.push(entry(
        "chat-notifications",
        Icon::Gear,
        tr("ui.notification_settings_view.notification_settings"),
        Action::Notifications {
            action: NotificationsAction::EditChat {
                conversation_id: id.clone(),
            },
        },
        cx,
    ));
    if row.can_leave {
        items.push(menu_separator(&theme).into_any_element());
        let title = row.title.clone();
        items.push(
            widgets::menu_item(
                "chat-leave",
                Some(Icon::Leave),
                tr("ui.chat_admin.leave"),
                &theme,
            )
            .text_color(theme.danger)
            .on_click(cx.listener(move |this, _, _, cx| {
                this.close_popup(cx);
                chat_admin::ask_leave(this, id.clone(), title.clone(), cx);
            }))
            .into_any_element(),
        );
    }
    items
}

/// ↑ / ↓ in the list: the chat above or below, in the order shown (favourites first).
fn move_selection(this: &mut Oxide, offset: i32) {
    let Some(chats) = this.sidebar.as_ref().map(|s| &s.chats) else {
        return;
    };
    if chats.is_empty() {
        return;
    }
    let ordered: Vec<&ChatRow> = chats
        .iter()
        .filter(|c| c.pinned)
        .chain(chats.iter().filter(|c| !c.pinned))
        .collect();
    let next = match ordered.iter().position(|c| c.selected) {
        Some(current) => current as i32 + offset,
        None if offset > 0 => 0,
        None => ordered.len() as i32 - 1,
    };
    let Some(chat) = usize::try_from(next).ok().and_then(|n| ordered.get(n)) else {
        return;
    };
    let id = chat.id.clone();
    if this.shell.as_ref().is_some_and(|s| s.nav != Nav::Chat) {
        this.send(Action::Navigate { nav: Nav::Chat });
    }
    let index = chats.iter().position(|c| c.id == id);
    if let Some(ix) = this
        .ui
        .sidebar
        .items
        .iter()
        .position(|i| matches!(i, Item::Chat(c) if Some(*c) == index))
    {
        this.ui.sidebar.list.state.scroll_to_reveal_item(ix);
    }
    this.send(Action::Select {
        target: Target { team: None, id },
    });
}

/// Grey bars standing in for list rows while the first page loads (`SkeletonList`).
pub fn skeleton(rows: usize, avatar: f32, theme: &Theme) -> gpui::Div {
    crate::widgets::skeleton("chats-skeleton", rows, avatar, theme)
}

/// Dialogs and floating panels of this module, drawn over the window.
pub fn overlay(
    this: &mut Oxide,
    _window: &mut Window,
    cx: &mut Context<Oxide>,
) -> Option<AnyElement> {
    if !this.popup_open(MENU) {
        return None;
    }
    let (id, position) = this.ui.sidebar.menu.clone()?;
    let row = this
        .sidebar
        .as_ref()?
        .chats
        .iter()
        .find(|c| c.id == id)?
        .clone();
    let theme = this.theme;
    let items = chat_actions(this, &row, cx);
    let entity = cx.entity().downgrade();
    Some(context_menu(
        position,
        div().flex().flex_col().children(items),
        &theme,
        move |_, cx| {
            let _ = entity.update(cx, |this, cx| this.close_popup(cx));
        },
    ))
}

/// A step of `OXIDE_SCRIPT` this module understands (see `screens::run_script`): `chats:filter <all|unread|meetings>`,
/// `chats:menu <n>` (context menu of the n-th chat shown), `chats:down`, `chats:up`, `chats:focus`.
pub fn debug_step(
    this: &mut Oxide,
    step: &str,
    window: &mut Window,
    cx: &mut Context<Oxide>,
) -> bool {
    let Some(rest) = step.strip_prefix("chats:") else {
        return false;
    };
    let (verb, arg) = rest.split_once(' ').unwrap_or((rest, ""));
    match verb {
        "filter" => {
            let filter = match arg {
                "unread" => Filter::Unread,
                "meetings" => Filter::Meetings,
                _ => Filter::All,
            };
            this.send(Action::SetFilter { filter });
        }
        "menu" => {
            let n: usize = arg.parse().unwrap_or(0);
            let Some(chat) = this
                .ui
                .sidebar
                .items
                .iter()
                .filter_map(|i| match i {
                    Item::Chat(c) => Some(*c),
                    _ => None,
                })
                .nth(n)
                .and_then(|c| this.sidebar.as_ref()?.chats.get(c).cloned())
            else {
                return false;
            };
            open_menu(
                this,
                chat.id,
                gpui::point(px(260.), px(200. + n as f32 * 56.)),
                cx,
            );
        }
        "focus" => window.focus(&this.ui.sidebar.focus),
        "down" => move_selection(this, 1),
        "up" => move_selection(this, -1),
        _ => return false,
    }
    true
}

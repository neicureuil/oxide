//! The open chat or channel (`ConversationScreen.swift`): header with calls and menus, tabs, the feed with its
//! thread pane, the typing indicator and the "new messages" pill.
use crate::feed::{self, FeedList, outline};
use crate::feed_message::{self, MessageMenu};
use crate::icons::{Icon, icon};
use crate::input::{InputEvent, TextInput};
use crate::oxide::{Oxide, text, tr, trf};
use crate::widgets::{self, ButtonKind};
use gpui::{
    Animation, AnimationExt, AnyElement, Context, Corner, Entity, FontWeight, SharedString,
    Subscription, Window, div, list, prelude::*, px,
};
use std::collections::HashMap;
use std::time::Duration;
use teams_core::{
    Action, CallAction, ChatAdminAction, ChatRow, ConversationView, NotificationsAction, Tab,
};

const CALLS_URL: &str = "https://teams.microsoft.com/v2/#/calls";

pub struct Ui {
    /// The message under the pointer, which shows its action bar.
    pub hovered: Option<String>,
    /// The right-click menu's message and position (popup `msg-menu`).
    pub menu: Option<MessageMenu>,
    pub confirm_delete: Option<String>,
    /// The chat whose leaving waits for confirmation.
    pub leaving: Option<ChatRow>,
    pub react_search: Entity<TextInput>,
    pub react_category: usize,
    /// Emoji ids last reacted with, newest first (this session only).
    pub recent_reactions: Vec<String>,
    /// Card values typed or picked, by `message/card/input`.
    pub card_values: HashMap<String, String>,
    pub card_inputs: HashMap<String, Entity<TextInput>>,
    /// Debug driver: every message shows its action bar.
    pub debug_hover_all: bool,
    _subscriptions: Vec<Subscription>,
}

impl Ui {
    pub fn new(_window: &mut Window, cx: &mut Context<Oxide>) -> Self {
        let react_search = cx.new(|cx| TextInput::new(cx, tr("ui.emoji_picker.search_emoji")));
        let subscriptions = vec![cx.subscribe(
            &react_search,
            |this: &mut Oxide, _, event: &InputEvent, cx| match event {
                InputEvent::Cancel => this.close_popup(cx),
                _ => cx.notify(),
            },
        )];
        Self {
            hovered: None,
            menu: None,
            confirm_delete: None,
            leaving: None,
            react_search,
            react_category: 0,
            recent_reactions: Vec::new(),
            card_values: HashMap::new(),
            card_inputs: HashMap::new(),
            debug_hover_all: false,
            _subscriptions: subscriptions,
        }
    }
}

/// Card inputs of messages that left the feeds go with them.
fn forget_gone_cards(this: &mut Oxide) {
    let ui = &mut this.ui.conversation;
    if ui.card_inputs.is_empty() && ui.card_values.is_empty() {
        return;
    }
    let (messages, posts, replies) = (&this.messages, &this.posts, &this.replies);
    let alive = |key: &String| {
        let message = key.split('/').next().unwrap_or_default();
        messages.contains(message)
            || replies.contains(message)
            || posts.contains(message)
            || posts.any_kept(|p| p.root.id == message)
    };
    ui.card_inputs.retain(|k, _| alive(k));
    ui.card_values.retain(|k, _| alive(k));
}

pub fn screen(this: &mut Oxide, window: &mut Window, cx: &mut Context<Oxide>) -> AnyElement {
    let theme = this.theme;
    // Texts register again as they are drawn below, for the selection.
    crate::feed_select::begin_frame();
    let Some(view) = this.conversation.clone() else {
        return div().size_full().bg(theme.conversation).into_any_element();
    };
    forget_gone_cards(this);
    let shell = this.shell.clone();
    let tab = shell.as_ref().map_or(Tab::Messages, |s| s.tab);
    let body = match tab {
        Tab::Files => crate::files::conversation_tab(this, window, cx),
        Tab::Members => crate::members::tab(this, window, cx),
        Tab::Messages => messages_tab(this, &view, window, cx),
    };
    div()
        .relative()
        .size_full()
        .flex()
        .flex_col()
        .bg(theme.conversation)
        .child(header(this, &view, tab, cx))
        .child(widgets::divider(&theme))
        // The tab's content fades in when the tab changes (`.animation(Motion.standard, value: tab)`).
        .child(crate::motion::fade_in(
            div().flex_1().min_h_0().child(body),
            SharedString::from(format!("conversation-body-{tab:?}")),
        ))
        .child(selection_listener(this, cx))
        .into_any_element()
}

/// Follows presses and drags over the messages' texts, to select across them (`FeedSelection`).
fn selection_listener(this: &Oxide, cx: &mut Context<Oxide>) -> impl IntoElement {
    let entity = cx.entity().downgrade();
    let core = this.core.clone();
    crate::feed_select::listener(
        move |cx| {
            let none = || crate::feed_select::Feed {
                lists: Vec::new(),
                plain: std::rc::Rc::new(|_: &str| Vec::new()),
            };
            let Some(this) = entity.upgrade() else {
                return none();
            };
            let this = this.read(cx);
            let channel = this.conversation.as_ref().is_some_and(|c| c.is_channel);
            // A post's texts are its root message's: order those by the posts' (posts come by the page).
            let posts = if channel { this.posts.views_of(&this.posts.ids()) } else { Vec::new() };
            let root_of: std::collections::HashMap<String, String> =
                posts.iter().map(|p| (p.root.id.clone(), p.id.clone())).collect();
            let main = if channel {
                posts.iter().map(|p| p.root.id.clone()).collect()
            } else {
                this.messages.ids()
            };
            // What a message scrolled away gives when it is copied whole: its author, then its text, asked of
            // the core then, for those between both ends only.
            let fetch_core = this.core.clone();
            let plain = move |id: &str| {
                let message = match root_of.get(id) {
                    Some(post) => fetch_core.post_views(std::slice::from_ref(post)).pop().map(|p| p.root),
                    None => fetch_core.message_views(&[id.to_owned()]).pop(),
                };
                message
                    .map(|m| vec![m.sender.clone(), crate::feed_blocks::body_text(&m.blocks)])
                    .unwrap_or_default()
            };
            crate::feed_select::Feed {
                lists: vec![main, this.replies.ids()],
                plain: std::rc::Rc::new(plain),
            }
        },
        move |url, _| core.send(Action::OpenUrl { url }),
    )
}

fn header(
    this: &mut Oxide,
    view: &ConversationView,
    tab: Tab,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    let theme = this.theme;
    let avatar = this.avatar(
        &view.target.id,
        &view.title,
        36.,
        view.avatar_shape,
        view.presence.as_ref(),
    );
    let members_core = this.core.clone();
    let members = outline(
        "conv-members",
        view.member_count.map(|n| n.to_string()).unwrap_or_default(),
        this,
    )
    .gap(px(6.))
    .child(icon(Icon::People, px(15.), theme.text_soft))
    .map(|d| {
        // The icon goes before the count, as SwiftUI's Label.
        let mut d = d;
        d.style().flex_direction = Some(gpui::FlexDirection::RowReverse);
        d
    })
    .tooltip(widgets::tooltip("Participants", theme))
    .on_click(move |_, _, _| members_core.send(Action::SetTab { tab: Tab::Members }));
    let bell_open = this.popup_open("conv-bell");
    let bell = div()
        .relative()
        .child(
            widgets::icon_button_sized(
                "conv-bell",
                if view.muted {
                    Icon::BellOff
                } else {
                    Icon::Bell
                },
                28.,
                15.,
                false,
                &theme,
            )
            .tooltip(widgets::tooltip("Notifications", theme))
            .on_click(cx.listener(|this, _, _, cx| this.toggle_popup("conv-bell", cx))),
        )
        .when(bell_open, |d| {
            d.child(popover(notification_menu(this, view, cx), this, cx))
        });
    let more = view.chat.clone().map(|chat| {
        let open = this.popup_open("conv-more");
        div()
            .relative()
            .child(
                widgets::icon_button_sized("conv-more", Icon::More, 28., 15., false, &theme)
                    .tooltip(widgets::tooltip(
                        tr("ui.conversation_screen.more_actions"),
                        theme,
                    ))
                    .on_click(cx.listener(|this, _, _, cx| this.toggle_popup("conv-more", cx))),
            )
            .when(open, |d| {
                d.child(popover(chat_actions(this, &chat, cx), this, cx))
            })
    });
    let tabs = [Tab::Messages, Tab::Files, Tab::Members];
    // The accent underline slides to the chosen tab (`matchedGeometryEffect`).
    let (underline, slid, measure) = crate::motion::sliding_selection(
        "conversation-tabs",
        tabs.iter().position(|t| *t == tab),
        0.,
        move |d| d.bottom_0().h(px(2.)).rounded_full().bg(theme.accent),
    );
    let tab_button = |label: String, value: Tab, this: &Oxide| {
        let active = tab == value;
        let core = this.core.clone();
        div()
            .id(SharedString::from(format!("conv-tab-{value:?}")))
            .flex()
            .flex_col()
            .gap(px(8.))
            .cursor_pointer()
            .child(
                div()
                    .pt(px(8.))
                    .text_size(px(13.))
                    .when(active, |d| d.font_weight(FontWeight::SEMIBOLD))
                    .text_color(if active { theme.text } else { theme.muted })
                    .child(label),
            )
            .child(
                div()
                    .h(px(2.))
                    .rounded_full()
                    .when(active && !slid, |d| d.bg(theme.accent)),
            )
            .on_click(move |_, _, _| core.send(Action::SetTab { tab: value }))
    };
    div()
        .flex_none()
        .px(px(20.))
        .bg(theme.surface)
        .flex()
        .flex_col()
        .child(
            div()
                .h(px(60.))
                .flex()
                .items_center()
                .gap(px(12.))
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
                                .truncate()
                                .text_size(px(17.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(view.title.clone()),
                        )
                        .child(
                            div()
                                .truncate()
                                .text_size(px(12.))
                                .text_color(theme.muted)
                                .child(view.subtitle.clone()),
                        ),
                )
                .children(call_buttons(this, view))
                .child(members)
                .child(bell)
                .children(more),
        )
        .child(
            div()
                .relative()
                .flex()
                .gap(px(20.))
                .on_children_prepainted(measure)
                .child(underline.unwrap_or_else(|| div().into_any_element()))
                .child(tab_button(
                    if view.is_channel {
                        tr("ui.conversation_screen.posts")
                    } else {
                        tr("ui.conversation_screen.chat")
                    },
                    Tab::Messages,
                    this,
                ))
                .child(tab_button(
                    if view.is_channel {
                        tr("ui.conversation_screen.files")
                    } else {
                        tr("ui.conversation_screen.shared")
                    },
                    Tab::Files,
                    this,
                ))
                .child(tab_button("Participants".into(), Tab::Members, this)),
        )
        .into_any_element()
}

/// `PrimaryButtonStyle` with a leading icon.
fn primary(
    id: &'static str,
    glyph: Icon,
    label: String,
    disabled: bool,
    this: &Oxide,
) -> gpui::Stateful<gpui::Div> {
    let theme = this.theme;
    let button = widgets::button(id, "", ButtonKind::Primary, &theme)
        .px(px(16.))
        .rounded(px(8.))
        .gap(px(6.))
        .font_weight(FontWeight::SEMIBOLD)
        .child(icon(glyph, px(15.), gpui::white()))
        .child(label);
    if disabled {
        button.bg(theme.accent.opacity(0.4)).cursor_default()
    } else {
        button
    }
}

/// Join (meeting chats) or call (other chats) natively, else in Teams on the web.
fn call_buttons(this: &mut Oxide, view: &ConversationView) -> Vec<AnyElement> {
    let theme = this.theme;
    let busy = this.in_call();
    let send = |action: Action, this: &Oxide| {
        let core = this.core.clone();
        move |_: &gpui::ClickEvent, _: &mut Window, _: &mut gpui::App| {
            if !busy {
                core.send(action.clone())
            }
        }
    };
    let phone = |id: &'static str, help: String, disabled: bool, this: &Oxide| {
        widgets::disabled(
            outline(id, "", this).child(icon(Icon::Phone, px(15.), theme.text_soft)),
            disabled,
        )
        .tooltip(widgets::tooltip(help, theme))
    };
    if view.is_meeting {
        vec![
            primary(
                "call-join",
                Icon::Video,
                tr("ui.conversation_screen.join"),
                busy,
                this,
            )
            .tooltip(widgets::tooltip(
                tr("ui.conversation_screen.join_the_meeting_in_oxide"),
                theme,
            ))
            .on_click(send(
                Action::Call {
                    action: CallAction::PrepareChatMeeting,
                },
                this,
            ))
            .into_any_element(),
        ]
    } else if view.can_call {
        let help = if view.member_count.unwrap_or(0) > 2 {
            tr("ui.conversation_screen.group_video_call")
        } else {
            tr("ui.conversation_screen.video_call")
        };
        vec![
            primary(
                "call-video",
                Icon::Video,
                tr("ui.conversation_screen.video_call"),
                busy,
                this,
            )
            .tooltip(widgets::tooltip(help, theme))
            .on_click(send(
                Action::Call {
                    action: CallAction::CallChat {
                        chat_id: None,
                        video: true,
                    },
                },
                this,
            ))
            .into_any_element(),
            phone(
                "call-audio",
                tr("ui.conversation_screen.audio_call"),
                busy,
                this,
            )
            .on_click(send(
                Action::Call {
                    action: CallAction::CallChat {
                        chat_id: None,
                        video: false,
                    },
                },
                this,
            ))
            .into_any_element(),
        ]
    } else {
        let open = |this: &Oxide| {
            let core = this.core.clone();
            move |_: &gpui::ClickEvent, _: &mut Window, _: &mut gpui::App| {
                core.send(Action::OpenUrl {
                    url: CALLS_URL.into(),
                })
            }
        };
        vec![
            primary(
                "call-meet",
                Icon::Video,
                tr("ui.conversation_screen.meet"),
                false,
                this,
            )
            .tooltip(widgets::tooltip(
                tr("ui.conversation_screen.meet_in_teams_on_the_web"),
                theme,
            ))
            .on_click(open(this))
            .into_any_element(),
            phone(
                "call-web",
                tr("ui.conversation_screen.audio_call_in_teams_on_the"),
                false,
                this,
            )
            .on_click(open(this))
            .into_any_element(),
        ]
    }
}

/// A header menu under its button; any item closes it.
fn popover(items: Vec<AnyElement>, this: &Oxide, cx: &mut Context<Oxide>) -> AnyElement {
    let theme = this.theme;
    let close = cx.listener(|this, _: &(), _, cx| this.close_popup(cx));
    widgets::popover(
        Corner::TopRight,
        div().flex().flex_col().children(items),
        &theme,
        move |w, cx| close(&(), w, cx),
    )
}

fn menu_action(
    id: &'static str,
    label: String,
    action: Action,
    this: &Oxide,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    let theme = this.theme;
    widgets::menu_item(id, None, label, &theme)
        .on_click(cx.listener(move |this, _, _, cx| {
            this.send(action.clone());
            this.close_popup(cx);
        }))
        .into_any_element()
}

/// `ChatNotificationMenu` or `ChannelNotificationMenu`.
fn notification_menu(
    this: &Oxide,
    view: &ConversationView,
    cx: &mut Context<Oxide>,
) -> Vec<AnyElement> {
    let id = view.target.id.clone();
    let notifications = |action| Action::Notifications { action };
    match (&view.target.team, view.is_channel) {
        (Some(team), true) => {
            let mut items = vec![menu_action(
                "bell-channel-settings",
                tr("ui.notification_settings_view.channel_notifications"),
                notifications(NotificationsAction::EditChannel {
                    team: team.clone(),
                    channel: id.clone(),
                }),
                this,
                cx,
            )];
            if !view.muted {
                items.push(menu_action(
                    "bell-channel-off",
                    tr("ui.notification_settings_view.turn_off_channel_notifications"),
                    notifications(NotificationsAction::MuteChannel {
                        team: team.clone(),
                        channel: id,
                    }),
                    this,
                    cx,
                ));
            }
            items
        }
        _ => chat_notification_items(this, &id, view.muted, cx),
    }
}

fn chat_notification_items(
    this: &Oxide,
    id: &str,
    muted: bool,
    cx: &mut Context<Oxide>,
) -> Vec<AnyElement> {
    let notifications = |action| Action::Notifications { action };
    vec![
        menu_action(
            "bell-chat-mute",
            if muted {
                tr("ui.notification_settings_view.unmute_notifications")
            } else {
                tr("ui.notification_settings_view.mute_notifications")
            },
            notifications(NotificationsAction::SetChatMuted {
                conversation_id: id.to_owned(),
                muted: !muted,
            }),
            this,
            cx,
        ),
        menu_action(
            "bell-chat-settings",
            tr("ui.notification_settings_view.notification_settings"),
            notifications(NotificationsAction::EditChat {
                conversation_id: id.to_owned(),
            }),
            this,
            cx,
        ),
    ]
}

/// `ChatActions`: what can be done to a chat, as from its row in the list.
fn chat_actions(this: &Oxide, row: &ChatRow, cx: &mut Context<Oxide>) -> Vec<AnyElement> {
    let theme = this.theme;
    let admin = |action| Action::ChatAdmin { action };
    let id = row.id.clone();
    let mut items = vec![
        menu_action(
            "chat-pin",
            if row.pinned {
                tr("ui.chat_admin.unpin")
            } else {
                tr("ui.chat_admin.pin")
            },
            admin(ChatAdminAction::SetPinned {
                chat_id: id.clone(),
                pinned: !row.pinned,
            }),
            this,
            cx,
        ),
        menu_action(
            "chat-read",
            if row.unread {
                tr("ui.chat_admin.mark_as_read")
            } else {
                tr("ui.chat_admin.mark_as_unread")
            },
            admin(if row.unread {
                ChatAdminAction::MarkRead {
                    chat_id: id.clone(),
                }
            } else {
                ChatAdminAction::MarkUnread {
                    chat_id: id.clone(),
                }
            }),
            this,
            cx,
        ),
        menu_action(
            "chat-hide",
            if row.hidden {
                tr("ui.chat_admin.show")
            } else {
                tr("ui.chat_admin.hide")
            },
            admin(if row.hidden {
                ChatAdminAction::Unhide {
                    chat_id: id.clone(),
                }
            } else {
                ChatAdminAction::Hide {
                    chat_id: id.clone(),
                }
            }),
            this,
            cx,
        ),
    ];
    items.extend(chat_notification_items(this, &id, row.muted, cx));
    if row.can_leave {
        let row = row.clone();
        items.push(div().my_1().h(px(1.)).bg(theme.border).into_any_element());
        items.push(
            widgets::menu_item("chat-leave", None, tr("ui.chat_admin.leave"), &theme)
                .text_color(theme.danger)
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.ui.conversation.leaving = Some(row.clone());
                    this.close_popup(cx);
                }))
                .into_any_element(),
        );
    }
    items
}

/// A feed of the open conversation, virtualised, with the "new messages" pill over its bottom.
fn feed_list<T: crate::feed::FeedEntry>(
    this: &mut Oxide,
    pick: fn(&Oxide) -> &FeedList<T>,
    row: fn(&mut Oxide, usize, &mut Window, &mut Context<Oxide>) -> AnyElement,
    id: &'static str,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    let feed = pick(this);
    let state = feed.list.clone();
    let (at_bottom, unseen) = (feed.at_bottom(), feed.unseen());
    let entity = cx.entity();
    let pill = (!at_bottom).then(|| new_messages_pill(this, id, unseen, pick, cx));
    div()
        .relative()
        .flex_1()
        .min_h_0()
        .w_full()
        .child(
            list(state, move |ix, window, cx| {
                entity.update(cx, |this, cx| {
                    // Pictures and cards size themselves from this feed's width (`feed::content_limit`).
                    crate::feed::drawing_feed(Some(id));
                    let drawn = row(this, ix, window, cx);
                    crate::feed::drawing_feed(None);
                    drawn
                })
            })
            .size_full()
            .py(px(8.)),
        )
        .children(pill)
        // Measures the feed: when its width changes (reply pane, window), the visible rows, which the list
        // lays out again every frame, are drawn once more at the new limit.
        .on_children_prepainted(move |bounds, window, _| {
            if let Some(b) = bounds.first()
                && crate::feed::set_feed_width(id, f32::from(b.size.width))
            {
                window.refresh();
            }
        })
        .into_any_element()
}

/// « ↓ 3 nouveaux messages » while the reader is scrolled up; a click goes back to the newest.
fn new_messages_pill<T: crate::feed::FeedEntry>(
    this: &Oxide,
    id: &'static str,
    unseen: usize,
    pick: fn(&Oxide) -> &FeedList<T>,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    let theme = this.theme;
    let label = match unseen {
        0 => tr("ui.feed_table.latest_messages"),
        1 => tr("ui.feed_table.1_new_message"),
        n => trf("ui.feed_table.new_messages", &[("unseen", &n.to_string())]),
    };
    let fg = if unseen == 0 {
        theme.text
    } else {
        gpui::white()
    };
    let pill = div()
                .id(SharedString::from(format!("{id}-pill")))
                .h(px(30.))
                .px(px(14.))
                .flex()
                .items_center()
                .gap(px(6.))
                .rounded_full()
                .cursor_pointer()
                .text_size(px(12.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(fg)
                .bg(if unseen == 0 {
                    theme.surface
                } else {
                    theme.accent
                })
                .when(unseen == 0, |d| d.border_1().border_color(theme.border))
                .shadow_md()
                .child(icon(Icon::ArrowDown, px(13.), fg))
                .child(label)
                .tooltip(widgets::tooltip(
                    tr("ui.feed_table.jump_to_latest_messages"),
                    theme,
                ))
                .on_click(cx.listener(move |this, _, _, cx| {
                    pick(this).jump_to_bottom();
                    cx.notify();
                }));
    // Rises from the bottom when the reader scrolls up (`transition(.rise)`); a new count pops.
    let pill = crate::motion::rise(pill, SharedString::from(format!("{id}-pill-in")));
    div()
        .absolute()
        .bottom(px(12.))
        .left_0()
        .right_0()
        .flex()
        .justify_center()
        .child(pill)
        .into_any_element()
}

/// A muted line where a feed has nothing to show (`FeedItem.note`), or the posts' error.
fn empty_feed(
    this: &Oxide,
    loading: bool,
    note: Option<String>,
    error: Option<String>,
) -> AnyElement {
    let theme = this.theme;
    let mut column = div()
        .flex_1()
        .min_h_0()
        .w_full()
        .flex()
        .flex_col()
        .justify_end();
    if loading {
        column = column.child(div().flex_1().flex().items_center().justify_center().child(
            widgets::loading("feed-loading", text("Chargement…", "Loading…"), &theme),
        ));
    } else if let Some(note) = note {
        column = column.child(
            div()
                .w_full()
                .p_4()
                .flex()
                .justify_center()
                .child(widgets::muted(note, 13., &theme)),
        );
    }
    if let Some(error) = error {
        column = column.child(feed::posts_error(error, this));
    }
    column.into_any_element()
}

fn messages_tab(
    this: &mut Oxide,
    view: &ConversationView,
    window: &mut Window,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    let theme = this.theme;
    let shell = this.shell.clone();
    let reply_open = shell.as_ref().is_some_and(|s| s.reply_open);
    let editing = shell.as_ref().is_some_and(|s| s.editing.is_some());
    let read_only = shell.as_ref().is_some_and(|s| s.read_only);
    let feed = if view.is_channel {
        if this.posts.is_empty() {
            empty_feed(
                this,
                view.loading,
                view.empty_text.clone(),
                view.posts_error.clone(),
            )
        } else {
            feed_list(this, |o| &o.posts, feed::post_row, "posts", cx)
        }
    } else if this.messages.is_empty() {
        empty_feed(this, view.loading, view.empty_text.clone(), None)
    } else {
        feed_list(this, |o| &o.messages, feed::chat_row, "messages", cx)
    };
    let mut column = div()
        .flex_1()
        .min_w_0()
        .h_full()
        .flex()
        .flex_col()
        .child(feed);
    if !view.is_channel {
        column = column.child(typing(this));
    }
    if reply_open {
        let core = this.core.clone();
        column = column.child(
            div().py(px(12.)).px(px(24.)).flex().child(
                outline("new-post", tr("ui.conversation_screen.new_post"), this)
                    .on_click(move |_, _, _| core.send(Action::CloseReply)),
            ),
        );
    } else if !editing {
        column = column.child(composer(this, false, read_only, window, cx));
    }
    let mut row = div().size_full().flex().child(column);
    if let Some(thread) = &view.thread {
        row = row
            .child(div().w(px(1.)).h_full().bg(theme.border))
            // The reply pane comes in from the right edge (`.move(edge: .trailing)`).
            .child({
                let panel = thread_panel(this, thread, editing, read_only, window, cx);
                crate::motion::slide_from_trailing(div().h_full().flex_none().child(panel), "thread-pane-in")
            });
    }
    row.into_any_element()
}

/// The compose box, or why there is none.
fn composer(
    this: &mut Oxide,
    thread: bool,
    read_only: bool,
    window: &mut Window,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    if read_only {
        let theme = this.theme;
        return div()
            .p(px(20.))
            .child(widgets::muted(
                tr("ui.composer.this_conversation_is_read_only"),
                12.,
                &theme,
            ))
            .into_any_element();
    }
    // Its own height: the feed above takes the rest.
    div()
        .flex_none()
        .child(crate::composer::composer(this, thread, window, cx))
        .into_any_element()
}

fn thread_panel(
    this: &mut Oxide,
    thread: &teams_core::ThreadView,
    editing: bool,
    read_only: bool,
    window: &mut Window,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    let theme = this.theme;
    let mut top = div().p(px(16.)).flex().flex_col().gap(px(12.));
    let mut heading = div().flex().items_center().child(
        div()
            .flex_1()
            .text_size(px(15.))
            .font_weight(FontWeight::SEMIBOLD)
            .child(tr("ui.conversation_screen.thread")),
    );
    if let Some(following) = thread.following {
        let core = this.core.clone();
        let post = thread.post.clone();
        heading = heading.child(
            widgets::icon_button_sized(
                "thread-follow",
                if following {
                    Icon::BellFill
                } else {
                    Icon::Bell
                },
                34.,
                16.,
                false,
                &theme,
            )
            .tooltip(widgets::tooltip(
                if following {
                    tr("ui.conversation_screen.unfollow_this_thread")
                } else {
                    tr("ui.conversation_screen.follow_this_thread")
                },
                theme,
            ))
            .on_click(move |_, _, _| {
                core.send(Action::Notifications {
                    action: NotificationsAction::SetPostFollow {
                        post: post.clone(),
                        follow: !following,
                    },
                })
            }),
        );
    }
    let core = this.core.clone();
    heading = heading.child(
        widgets::icon_button_sized("thread-close", Icon::Close, 34., 16., false, &theme)
            .tooltip(widgets::tooltip(
                tr("ui.conversation_screen.close_thread"),
                theme,
            ))
            .on_click(move |_, _, _| core.send(Action::CloseReply)),
    );
    top = top.child(heading);
    if !thread.root_preview.is_empty() || !thread.root_sender.is_empty() {
        let avatar = this.avatar(
            &thread.root_sender_id,
            &thread.root_sender,
            24.,
            thread.root_sender_avatar_shape,
            None,
        );
        top = top.child(
            div()
                .p(px(10.))
                .rounded(px(8.))
                .bg(theme.input)
                .flex()
                .items_start()
                .gap(px(8.))
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
                                .truncate()
                                .flex()
                                .items_baseline()
                                .child(
                                    div()
                                        .text_size(px(12.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .child(thread.root_sender.clone()),
                                )
                                .when(!thread.root_time.is_empty(), |d| {
                                    d.child(
                                        div()
                                            .text_size(px(11.))
                                            .text_color(theme.muted)
                                            .child(format!(" · {}", thread.root_time)),
                                    )
                                }),
                        )
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(theme.muted)
                                .line_clamp(4)
                                .child(thread.root_preview.clone()),
                        ),
                ),
        );
    }
    let replies = if this.replies.is_empty() {
        empty_feed(this, thread.loading, thread.empty_text.clone(), None)
    } else {
        feed_list(this, |o| &o.replies, feed::reply_row, "replies", cx)
    };
    div()
        .w(px(360.))
        .flex_none()
        .h_full()
        .flex()
        .flex_col()
        .bg(theme.surface)
        .child(top)
        .child(widgets::divider(&theme))
        .child(replies)
        .when(!editing, |d| {
            d.child(composer(this, true, read_only, window, cx))
        })
        .into_any_element()
}

/// « Alice est en train d'écrire » under the messages, with three bouncing dots.
fn typing(this: &Oxide) -> AnyElement {
    let theme = this.theme;
    let row = div()
        .min_h(px(20.))
        .px(px(24.))
        .pt(px(2.))
        .flex()
        .items_center()
        .gap_2();
    let Some(text) = this.typing.clone() else {
        return row.into_any_element();
    };
    let dots = div()
        .h(px(10.))
        .flex()
        .items_end()
        .gap(px(3.))
        .children((0..3).map(|ix| {
            div()
                .size(px(5.))
                .rounded_full()
                .bg(theme.muted)
                .with_animation(
                    SharedString::from(format!("typing-dot-{ix}")),
                    Animation::new(Duration::from_millis(1200)).repeat(),
                    move |dot, delta| dot.mb(px(-dot_offset(delta * 1.2, ix))),
                )
        }));
    // Comes and goes with a fade and a slight lift (`.opacity.combined(with: .offset(y: 4))`).
    let line = row.child(dots).child(
        div()
            .truncate()
            .text_size(px(12.))
            .text_color(theme.muted)
            .child(text),
    );
    crate::motion::animate(line, "typing-in", crate::motion::Curve::Standard, |d, t| {
        d.opacity(t).mt(px(4. * (1. - t)))
    })
}

/// Each dot rises for a third of a 1.2 s cycle, the next one starting 0.15 s later (`TypingDots.offset`).
pub fn dot_offset(time: f32, index: usize) -> f32 {
    let phase = (time - index as f32 * 0.15).rem_euclid(1.2);
    if phase >= 0.4 {
        return 0.;
    }
    -3. * (phase / 0.4 * std::f32::consts::PI).sin()
}

/// The leave confirmation (`ConversationScreen`'s dialog).
fn leave_dialog(this: &mut Oxide, cx: &mut Context<Oxide>) -> Option<AnyElement> {
    let row = this.ui.conversation.leaving.clone()?;
    let theme = this.theme;
    let cancel = cx.listener(|this, _: &gpui::ClickEvent, _, cx| {
        this.ui.conversation.leaving = None;
        cx.notify();
    });
    let id = row.id.clone();
    let leave = cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
        this.ui.conversation.leaving = None;
        this.send(Action::ChatAdmin {
            action: ChatAdminAction::Leave {
                chat_id: id.clone(),
            },
        });
        cx.notify();
    });
    let close = cx.listener(|this, _: &(), _, cx| {
        this.ui.conversation.leaving = None;
        cx.notify();
    });
    Some(widgets::modal(
        "leave-chat",
        px(360.),
        div()
            .p_5()
            .flex()
            .flex_col()
            .gap_3()
            .child(widgets::title(
                trf("ui.chat_admin.leave_question", &[("title", &row.title)]),
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
                        outline("leave-cancel", tr("ui.message_row.cancel"), this).on_click(cancel),
                    )
                    .child(
                        widgets::button(
                            "leave-confirm",
                            tr("ui.chat_admin.leave"),
                            ButtonKind::Danger,
                            &theme,
                        )
                        .rounded(px(8.))
                        .on_click(leave),
                    ),
            ),
        &theme,
        move |w, cx| close(&(), w, cx),
    ))
}

/// Dialogs and floating panels of this module, drawn over the window.
pub fn overlay(
    this: &mut Oxide,
    _window: &mut Window,
    cx: &mut Context<Oxide>,
) -> Option<AnyElement> {
    if this.conversation.is_none() {
        return None;
    }
    let all: Vec<AnyElement> = [
        feed_message::context_menu(this, cx),
        feed_message::delete_dialog(this, cx),
        leave_dialog(this, cx),
    ]
    .into_iter()
    .flatten()
    .collect();
    if all.is_empty() {
        return None;
    }
    Some(div().absolute().inset_0().children(all).into_any_element())
}

/// Steps of `OXIDE_SCRIPT` for this screen (see `screens::run_script`):
/// `conv:front` (brings the window forward), `conv:open:<title part>` (opens that chat), `conv:channel:<n>`, `conv:hover` (every message shows its actions), `conv:react:<n>` (picker of the n-th message), `conv:more:<n>`,
/// `conv:menu:<n>` (right-click menu), `conv:edit:<n>` (edit my n-th message), `conv:delete:<n>`,
/// `conv:tab:<messages|files|members>`, `conv:bell`, `conv:actions`, `conv:leave`, `conv:post:<n>` (open the n-th
/// post's thread), `conv:top` (scroll to the oldest), `conv:pill` (scroll up a little).
pub fn debug_step(
    this: &mut Oxide,
    step: &str,
    window: &mut Window,
    cx: &mut Context<Oxide>,
) -> bool {
    if step == "conv:select" {
        let lists = vec![this.messages.ids(), this.replies.ids()];
        let done = crate::feed_select::debug_select(lists);
        if let Some(text) = crate::feed_select::text() {
            eprintln!("OXIDE_SCRIPT: selected {text:?}");
        }
        cx.notify();
        let _ = window;
        return done;
    }
    let Some(rest) = step.strip_prefix("conv:") else {
        return false;
    };
    let (verb, arg) = rest.split_once(':').unwrap_or((rest, ""));
    let n: usize = arg.parse().unwrap_or(0);
    let channel = this.conversation.as_ref().is_some_and(|c| c.is_channel);
    let nth = |this: &Oxide| -> Option<teams_core::MessageView> {
        if channel {
            this.posts.get(n).map(|p| p.root.clone())
        } else {
            this.messages.get(n)
        }
    };
    match verb {
        "open" => {
            // By title, as the chat list's order changes with every message.
            let Some(chat) = this
                .sidebar
                .as_ref()
                .and_then(|s| s.chats.iter().find(|c| c.title.contains(arg)).cloned())
            else {
                return false;
            };
            this.send(Action::Select {
                target: teams_core::Target {
                    team: None,
                    id: chat.id,
                },
            });
        }
        "channel" => {
            // The n-th channel of the teams list, with the Teams section open.
            let Some(channel) = this.sidebar.as_ref().and_then(|s| {
                s.teams
                    .iter()
                    .flat_map(|t| t.channels.iter())
                    .nth(n)
                    .cloned()
            }) else {
                return false;
            };
            this.send(Action::Navigate {
                nav: teams_core::Nav::Teams,
            });
            this.send(Action::Select {
                target: teams_core::Target {
                    team: Some(channel.team),
                    id: channel.id,
                },
            });
        }
        "hover" => this.ui.conversation.debug_hover_all = true,
        // Other windows may cover this one, which then stops drawing: bring it forward before a capture.
        "front" => {
            cx.activate(true);
            window.activate_window();
        }
        "react" | "more" => {
            let Some(msg) = nth(this) else { return false };
            this.ui.conversation.hovered = Some(msg.id.clone());
            this.popup = Some(
                if verb == "react" {
                    format!("react:{}", msg.id)
                } else {
                    format!("msg-more:{}", msg.id)
                }
                .into(),
            );
        }
        "menu" => {
            let Some(msg) = nth(this) else { return false };
            let kind = if channel {
                feed_message::RowKind::Post(
                    this.posts.get(n).map(|p| p.id.clone()).unwrap_or_default(),
                )
            } else {
                feed_message::RowKind::Bubble
            };
            this.ui.conversation.menu = Some(MessageMenu {
                message: msg,
                kind,
                at: gpui::point(px(700.), px(300.)),
            });
            this.popup = Some(feed_message::MENU_KEY.into());
        }
        "edit" => {
            let Some(msg) = (0..this.messages.len())
                .filter_map(|i| this.messages.get(i))
                .filter(|m| m.mine)
                .nth(n)
            else {
                return false;
            };
            this.send(Action::Edit { id: msg.id });
        }
        "delete" => {
            let Some(msg) = nth(this) else { return false };
            this.ui.conversation.confirm_delete = Some(msg.id);
        }
        "tab" => {
            let tab = match arg {
                "files" => Tab::Files,
                "members" => Tab::Members,
                _ => Tab::Messages,
            };
            this.send(Action::SetTab { tab });
        }
        "bell" => this.popup = Some("conv-bell".into()),
        "actions" => this.popup = Some("conv-more".into()),
        "leave" => {
            this.ui.conversation.leaving = this.conversation.as_ref().and_then(|c| c.chat.clone())
        }
        "post" => {
            let Some(post) = this.posts.get(n).map(|p| p.id.clone()) else {
                return false;
            };
            this.send(Action::OpenReply { post });
        }
        "top" => {
            let list = if channel {
                &this.posts.list
            } else {
                &this.messages.list
            };
            list.scroll_to(gpui::ListOffset {
                item_ix: 0,
                offset_in_item: px(0.),
            });
        }
        "pill" => {
            let list = if channel {
                &this.posts.list
            } else {
                &this.messages.list
            };
            list.scroll_by(px(-300.));
        }
        _ => return false,
    }
    cx.notify();
    true
}

#[cfg(test)]
mod tests {
    use super::dot_offset;

    #[test]
    fn dots_bounce_in_turn() {
        assert_eq!(dot_offset(0., 0), 0.);
        assert!((dot_offset(0.2, 0) + 3.).abs() < 1e-4);
        assert_eq!(dot_offset(0.2, 2), 0.);
        assert!(dot_offset(0.5, 2) < 0.);
        assert_eq!(dot_offset(0.5, 0), 0.);
    }
}

//! The activity column and overview (`ActivityView.swift`): the feed, each item's badge and missed call
//! actions, and the selected item's details (a meeting's event, answer and attendees).
use crate::chats_parts::{SyncedList, sig};
use crate::icons::{Icon, icon};
use crate::oxide::{Oxide, english, tr};
use crate::sidebar;
use crate::theme::Theme;
use crate::widgets::{self, ButtonKind};
use gpui::{
    AnyElement, Context, Corner, FontWeight, Hsla, SharedString, Window, div, list, prelude::*, px,
    rgb,
};
use teams_core::{
    Action, ActivityBadge, ActivityRow, AvatarShape, CalendarAction, CalendarEventView, CallAction,
    Filter, Nav, Target,
};

pub struct Ui {
    list: SyncedList,
    /// What the list shows after the rows.
    tail: Vec<Tail>,
}

impl Ui {
    pub fn new(_window: &mut Window, _cx: &mut Context<Oxide>) -> Self {
        Self {
            list: SyncedList::new(),
            tail: Vec::new(),
        }
    }
}

#[derive(Clone, Copy, Hash)]
enum Tail {
    Error,
    More,
}

const RSVP: &str = "activity-rsvp";

pub fn sidebar(this: &mut Oxide, _window: &mut Window, cx: &mut Context<Oxide>) -> AnyElement {
    let theme = this.theme;
    let Some(shell) = this.shell.clone() else {
        return div().into_any_element();
    };
    let column = div()
        .size_full()
        .flex()
        .flex_col()
        .bg(theme.surface)
        .child(sidebar::header(this, &shell, cx));
    if this.sidebar.is_none() {
        return column.into_any_element();
    }
    column
        .child(sidebar::chips(this, &shell))
        .child(activity_list(this, shell.filter == Filter::Unread, cx))
        .into_any_element()
}

fn activity_list(this: &mut Oxide, unread_only: bool, cx: &mut Context<Oxide>) -> AnyElement {
    let theme = this.theme;
    let Some(feed) = this.sidebar.as_ref() else {
        return div().into_any_element();
    };
    let mut tail = Vec::new();
    if feed.activity_error.is_some() {
        tail.push(Tail::Error);
    }
    if feed.more_activity {
        tail.push(Tail::More);
    }
    if feed.activities.is_empty() {
        this.ui.activity.list.clear();
        let mut column = div().flex_1().p_2().flex().flex_col();
        if feed.activity_error.is_none() {
            column = column.child(if feed.activity_loading {
                sidebar::skeleton(7, 36., &theme).into_any_element()
            } else {
                div()
                    .p_4()
                    .child(widgets::muted(
                        tr(if unread_only {
                            "ui.activity_view.no_unread_notifications"
                        } else {
                            "ui.activity_view.no_notifications_to_show"
                        }),
                        12.,
                        &theme,
                    ))
                    .into_any_element()
            });
        }
        this.ui.activity.tail = tail.clone();
        for item in tail {
            column = column.child(tail_item(this, item, cx));
        }
        return column.into_any_element();
    }
    let loading = feed.activity_loading;
    let mut sigs: Vec<u64> = feed.activities.iter().map(sig).collect();
    sigs.extend(tail.iter().map(|t| sig(&(t, loading))));
    this.ui.activity.tail = tail;
    this.ui.activity.list.sync(sigs);
    let entity = cx.entity();
    list(this.ui.activity.list.state.clone(), move |ix, _, cx| {
        entity.update(cx, |this, cx| item(this, ix, cx))
    })
    .flex_1()
    .pt_2()
    .into_any_element()
}

fn item(this: &mut Oxide, ix: usize, cx: &mut Context<Oxide>) -> AnyElement {
    let count = this.sidebar.as_ref().map_or(0, |s| s.activities.len());
    let body = if ix < count {
        let Some(row) = this
            .sidebar
            .as_ref()
            .and_then(|s| s.activities.get(ix))
            .cloned()
        else {
            return div().into_any_element();
        };
        activity_row(this, &row, cx)
    } else {
        match this.ui.activity.tail.get(ix - count).copied() {
            Some(t) => tail_item(this, t, cx),
            None => return div().into_any_element(),
        }
    };
    div().px_2().pb_1().child(body).into_any_element()
}

fn tail_item(this: &mut Oxide, item: Tail, cx: &mut Context<Oxide>) -> AnyElement {
    let theme = this.theme;
    let Some(feed) = this.sidebar.as_ref() else {
        return div().into_any_element();
    };
    match item {
        Tail::Error => div()
            .flex()
            .flex_col()
            .items_start()
            .child(div().p(px(12.)).child(widgets::muted(
                feed.activity_error.clone().unwrap_or_default(),
                12.,
                &theme,
            )))
            .child(
                ghost(
                    "activity-retry",
                    tr("ui.activity_view.try_again"),
                    13.,
                    false,
                    &theme,
                )
                .on_click(cx.listener(|this, _, _, _| this.send(Action::Refresh))),
            )
            .into_any_element(),
        Tail::More => {
            let loading = feed.activity_loading;
            let label = tr(if loading {
                "ui.activity_view.loading"
            } else {
                "ui.activity_view.earlier_notifications"
            });
            div()
                .p_2()
                .flex()
                .child(
                    ghost("activity-more", label, 12., loading, &theme).on_click(cx.listener(
                        move |this, _, _, _| {
                            if !loading {
                                this.send(Action::MoreActivity);
                            }
                        },
                    )),
                )
                .into_any_element()
        }
    }
}

/// A borderless button in muted text (`GhostButtonStyle`).
fn ghost(
    id: &'static str,
    label: String,
    size: f32,
    disabled: bool,
    theme: &Theme,
) -> gpui::Stateful<gpui::Div> {
    let hover = theme.input;
    div()
        .id(id)
        .px_2()
        .py_1()
        .rounded(px(8.))
        .text_size(px(size))
        .text_color(theme.muted)
        .when(!disabled, |d| {
            d.cursor_pointer().hover(move |s| s.bg(hover))
        })
        .when(disabled, |d| d.opacity(0.45))
        .child(label)
}

/// One feed item as Teams draws it, with a missed call's actions under it.
fn activity_row(this: &mut Oxide, row: &ActivityRow, cx: &mut Context<Oxide>) -> AnyElement {
    let theme = this.theme;
    let avatar = this.avatar(&row.actor_id, &row.actor, 36., AvatarShape::Round, None);
    let hover = theme.input;
    let id = row.id.clone();
    let mut content = div()
        .id(SharedString::from(format!("activity-{}", row.id)))
        .flex()
        .items_start()
        .gap(px(10.))
        .pt(px(10.))
        .px(px(10.))
        .pb(px(if row.call_back.is_some() { 4. } else { 10. }))
        .cursor_pointer()
        .child(
            div().relative().flex_none().child(avatar).children(
                badge(row.badge, row.reaction.as_deref(), &theme)
                    .map(|b| div().absolute().right(px(-4.)).bottom(px(-4.)).child(b)),
            ),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(3.))
                .child(
                    div()
                        .flex()
                        .items_start()
                        .gap_2()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_size(px(13.))
                                .line_height(px(17.))
                                .font_weight(if row.unread {
                                    FontWeight::SEMIBOLD
                                } else {
                                    FontWeight::MEDIUM
                                })
                                .line_clamp(2)
                                .text_ellipsis()
                                .child(row.title.clone()),
                        )
                        .child(
                            div()
                                .flex_none()
                                .pt(px(2.))
                                .text_size(px(11.))
                                .text_color(theme.subtle)
                                .child(row.short_time.clone()),
                        ),
                )
                .when(!row.context.is_empty(), |d| {
                    d.child(
                        div()
                            .line_clamp(1)
                            .text_ellipsis()
                            .text_size(px(12.))
                            .text_color(theme.text_soft)
                            .child(row.context.clone()),
                    )
                })
                .when(!row.preview.is_empty(), |d| {
                    d.child(
                        div()
                            .line_clamp(1)
                            .text_ellipsis()
                            .text_size(px(12.))
                            .text_color(theme.muted)
                            .child(row.preview.clone()),
                    )
                }),
        )
        .when(row.unread, |d| {
            d.child(
                div()
                    .flex_none()
                    .mt(px(5.))
                    .size(px(8.))
                    .rounded_full()
                    .bg(theme.accent),
            )
        })
        .on_click(
            cx.listener(move |this, _, _, _| this.send(Action::OpenActivity { id: id.clone() })),
        )
        .into_any_element();
    if let Some(mri) = row.call_back.clone() {
        let chat = row.chat_id.clone();
        let small = |button: gpui::Stateful<gpui::Div>| {
            button.text_size(px(12.)).font_weight(FontWeight::SEMIBOLD)
        };
        content = div()
            .flex()
            .flex_col()
            .child(content)
            .child(
                div()
                    .flex()
                    .gap_2()
                    // Lined up with the text, past the avatar.
                    .pl(px(56.))
                    .pr(px(10.))
                    .pb(px(10.))
                    .child(
                        small(widgets::button(
                            SharedString::from(format!("call-back-{}", row.id)),
                            tr("ui.activity_view.call"),
                            ButtonKind::Outline,
                            &theme,
                        ))
                        .on_click(cx.listener(move |this, _, _, _| {
                            this.send(Action::Call {
                                action: CallAction::Start {
                                    mri: mri.clone(),
                                    video: false,
                                },
                            })
                        })),
                    )
                    .children(chat.map(|chat| {
                        small(widgets::button(
                            SharedString::from(format!("call-chat-{}", row.id)),
                            tr("ui.activity_view.chat"),
                            ButtonKind::Outline,
                            &theme,
                        ))
                        .on_click(cx.listener(move |this, _, _, _| open_chat(this, chat.clone())))
                    })),
            )
            .into_any_element();
    }
    let line = div()
        .id(SharedString::from(format!("activity-line-{}", row.id)))
        .rounded(px(8.))
        .when(row.selected, |d| d.bg(theme.accent_soft));
    let line = if row.selected {
        line
    } else {
        crate::motion::hover_fill(line, format!("activity-line-{}", row.id), hover, px(8.))
    };
    line.child(content).into_any_element()
}

fn orange(theme: &Theme) -> Hsla {
    rgb(if theme.dark { 0xFF9F0A } else { 0xFF9500 }).into()
}
fn green(theme: &Theme) -> Hsla {
    rgb(if theme.dark { 0x30D158 } else { 0x34C759 }).into()
}

/// Small chip on the avatar: what kind of activity this is (`ActivityBadgeView`).
fn badge(badge: ActivityBadge, reaction: Option<&str>, theme: &Theme) -> Option<AnyElement> {
    let glyph = match badge {
        ActivityBadge::None => return None,
        ActivityBadge::Mention => Icon::At,
        ActivityBadge::Reply => Icon::Reply,
        ActivityBadge::Reaction => Icon::Smile,
        ActivityBadge::MissedCall => Icon::Phone,
        ActivityBadge::Voicemail => Icon::Voicemail,
        ActivityBadge::Meeting => Icon::Calendar,
        ActivityBadge::App => Icon::Apps,
        ActivityBadge::Team => Icon::People,
        ActivityBadge::Urgent => Icon::Alert,
    };
    let color = match badge {
        ActivityBadge::Mention => orange(theme),
        ActivityBadge::MissedCall | ActivityBadge::Urgent => theme.danger,
        ActivityBadge::Reaction => theme.input,
        _ => theme.accent,
    };
    let chip = div()
        .size(px(18.))
        .rounded_full()
        .border_2()
        .border_color(theme.surface)
        .bg(color)
        .flex()
        .items_center()
        .justify_center();
    Some(
        match (badge, reaction) {
            (ActivityBadge::Reaction, Some(emoji)) => chip
                .text_size(px(9.))
                .line_height(px(10.))
                .child(emoji.to_owned()),
            _ => chip.child(icon(glyph, px(10.), gpui::white())),
        }
        .into_any_element(),
    )
}

fn open_chat(this: &mut Oxide, id: String) {
    this.send(Action::Navigate { nav: Nav::Chat });
    this.send(Action::Select {
        target: Target { team: None, id },
    });
}

/// The selected notification when it opens nothing else: its text, or a meeting's details.
pub fn overview(this: &mut Oxide, _window: &mut Window, cx: &mut Context<Oxide>) -> AnyElement {
    let theme = this.theme;
    let row = this
        .sidebar
        .as_ref()
        .and_then(|s| s.activities.iter().find(|a| a.selected))
        .cloned();
    if let Some(row) = &row
        && row.meeting
    {
        return meeting_pane(this, row, cx);
    }
    let mut column = div()
        .max_w(px(700.))
        .p(px(32.))
        .flex()
        .flex_col()
        .gap_4()
        .child(widgets::title(
            row.as_ref()
                .map_or_else(|| tr("ui.activity_view.activity"), |r| r.title.clone()),
            24.,
        ));
    match &row {
        Some(row) => {
            if !row.context.is_empty() {
                column = column.child(
                    div()
                        .text_size(px(16.))
                        .font_weight(FontWeight::MEDIUM)
                        .child(row.context.clone()),
                );
            }
            column = column
                .child(widgets::muted(row.time.clone(), 12., &theme))
                .child(
                    div()
                        .text_size(px(15.))
                        .line_height(px(21.))
                        .child(row.preview.clone()),
                );
        }
        None => {
            column = column.child(widgets::muted(
                tr("ui.activity_view.select_a_notification_to_open_it"),
                14.,
                &theme,
            ));
        }
    }
    div()
        .id("activity-overview")
        .size_full()
        .overflow_y_scroll()
        .child(column)
        .into_any_element()
}

/// Teams' "Détails" of a meeting notification: the event, its answer, and its attendees.
fn meeting_pane(this: &mut Oxide, row: &ActivityRow, cx: &mut Context<Oxide>) -> AnyElement {
    let theme = this.theme;
    let event = row
        .event_id
        .as_ref()
        .and_then(|id| {
            this.calendar
                .as_ref()?
                .selected
                .as_ref()
                .filter(|e| &e.id == id)
        })
        .cloned();
    let loading = this.calendar.as_ref().is_some_and(|c| c.loading_detail);
    let mut details = div()
        .max_w(px(720.))
        .p(px(28.))
        .flex()
        .flex_col()
        .gap(px(18.))
        .child(meeting_header(this, row, event.as_ref(), cx));
    if let Some(event) = &event {
        details = details.children(event_details(this, row, event, cx));
    } else if loading {
        details = details.child(div().flex().justify_center().child(widgets::spinner(
            "meeting-loading",
            16.,
            theme.muted,
        )));
    } else {
        if !row.context.is_empty() {
            details = details.child(label(Icon::Calendar, row.context.clone(), 13., &theme));
        }
        let note = this
            .calendar
            .as_ref()
            .and_then(|c| c.error.clone())
            .unwrap_or_else(|| tr("ui.activity_view.meeting_details_aren_t_available"));
        details = details.child(widgets::muted(note, 13., &theme));
    }
    div()
        .size_full()
        .flex()
        .child(
            div()
                .id("meeting-details")
                .flex_1()
                .min_w_0()
                .h_full()
                .overflow_y_scroll()
                .child(details),
        )
        .when_some(event, |d, event| {
            d.child(div().w(px(1.)).h_full().bg(theme.border))
                .child(attendees(this, &event))
        })
        .into_any_element()
}

fn meeting_header(
    this: &mut Oxide,
    row: &ActivityRow,
    event: Option<&CalendarEventView>,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    let theme = this.theme;
    let subject = event
        .map(|e| e.form.subject.clone())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| row.preview.clone());
    let chat = row.chat_id.clone();
    let join = event
        .filter(|e| !e.join_url.is_empty() && !e.cancelled)
        .map(|e| (e.join_url.clone(), subject.clone()));
    div()
        .flex()
        .items_center()
        .gap(px(12.))
        .child(
            div()
                .size(px(34.))
                .flex_none()
                .rounded(px(8.))
                .bg(theme.accent)
                .flex()
                .items_center()
                .justify_center()
                .child(icon(Icon::Calendar, px(17.), gpui::white())),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .text_size(px(20.))
                .line_height(px(26.))
                .font_weight(FontWeight::SEMIBOLD)
                .line_clamp(2)
                .text_ellipsis()
                .child(subject),
        )
        .children(chat.map(|chat| {
            widgets::button(
                "meeting-chat",
                tr("ui.activity_view.chat"),
                ButtonKind::Outline,
                &theme,
            )
            .on_click(cx.listener(move |this, _, _, _| open_chat(this, chat.clone())))
        }))
        .children(join.map(|(url, title)| {
            widgets::button(
                "meeting-join",
                tr("ui.activity_view.join"),
                ButtonKind::Primary,
                &theme,
            )
            .on_click(cx.listener(move |this, _, _, _| {
                this.send(Action::Call {
                    action: CallAction::PrepareMeeting {
                        url: url.clone(),
                        title: title.clone(),
                    },
                })
            }))
        }))
        .into_any_element()
}

fn event_details(
    this: &mut Oxide,
    row: &ActivityRow,
    event: &CalendarEventView,
    cx: &mut Context<Oxide>,
) -> Vec<AnyElement> {
    let theme = this.theme;
    let mut out = Vec::new();
    if event.cancelled {
        out.push(
            div()
                .flex()
                .items_center()
                .gap_2()
                .text_size(px(13.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.danger)
                .child(icon(Icon::Close, px(14.), theme.danger))
                .child(tr("ui.activity_view.this_meeting_has_been_canceled"))
                .into_any_element(),
        );
    }
    if event.can_respond {
        let busy = this.calendar.as_ref().is_some_and(|c| c.busy);
        let open = this.popup_open(RSVP);
        let entity = cx.entity().downgrade();
        let mut menu = div().flex().flex_col();
        for (key, response) in [
            ("ui.activity_view.accept", "Accept"),
            ("ui.activity_view.tentative", "TentativelyAccept"),
            ("ui.activity_view.decline", "Decline"),
        ] {
            menu = menu.child(
                widgets::menu_item(
                    SharedString::from(format!("rsvp-{response}")),
                    None,
                    tr(key),
                    &theme,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.send(Action::Calendar {
                        action: CalendarAction::Respond {
                            response: response.to_owned(),
                            series: false,
                        },
                    });
                    this.close_popup(cx);
                })),
            );
        }
        out.push(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .child(
                            widgets::disabled(
                                widgets::button("rsvp", "", ButtonKind::Outline, &theme)
                                    .child(icon(Icon::Reply, px(14.), theme.text_soft))
                                    .child(tr("ui.activity_view.rsvp"))
                                    .child(icon(Icon::ChevronDown, px(11.), theme.muted)),
                                busy,
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    if !busy {
                                        this.toggle_popup(RSVP, cx);
                                    }
                                },
                            )),
                        )
                        .when(open, |d| {
                            d.child(widgets::popover(
                                Corner::TopLeft,
                                menu,
                                &theme,
                                move |_, cx| {
                                    let _ = entity.update(cx, |this, cx| this.close_popup(cx));
                                },
                            ))
                        }),
                )
                .when(!event.response.is_empty(), |d| {
                    d.child(widgets::muted(
                        format!(
                            "{}{}",
                            tr("ui.activity_view.your_response"),
                            response_label(&event.response)
                        ),
                        12.,
                        &theme,
                    ))
                })
                .into_any_element(),
        );
    }
    let when = if row.context.is_empty() {
        range(event)
    } else {
        row.context.clone()
    };
    out.push(
        div()
            .flex()
            .flex_col()
            .gap_3()
            .child(label(Icon::Clock, when, 14., &theme))
            .when(!event.form.location.is_empty(), |d| {
                d.child(label(
                    Icon::Location,
                    event.form.location.clone(),
                    14.,
                    &theme,
                ))
            })
            .when(event.recurring, |d| {
                d.child(label(
                    Icon::Repeat,
                    tr("ui.activity_view.recurring_meeting"),
                    14.,
                    &theme,
                ))
            })
            .into_any_element(),
    );
    if !event.form.description.is_empty() {
        out.push(widgets::divider(&theme).into_any_element());
        out.push(
            div()
                .text_size(px(13.))
                .child(event.form.description.clone())
                .into_any_element(),
        );
    }
    out.push(
        div()
            .flex()
            .child(
                div()
                    .id("open-in-calendar")
                    .cursor_pointer()
                    .text_color(theme.accent_text)
                    .hover(|s| s.underline())
                    .child(tr("ui.activity_view.open_in_calendar"))
                    .on_click(cx.listener(|this, _, _, _| {
                        this.send(Action::Navigate { nav: Nav::Calendar })
                    })),
            )
            .into_any_element(),
    );
    out
}

/// An icon and its text, as SwiftUI's `Label`.
fn label(glyph: Icon, text: String, size: f32, theme: &Theme) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap_2()
        .text_size(px(size))
        .child(icon(glyph, px(size + 2.), theme.muted))
        .child(text)
        .into_any_element()
}

fn attendees(this: &mut Oxide, event: &CalendarEventView) -> AnyElement {
    let theme = this.theme;
    let mut column = div()
        .p(px(20.))
        .flex()
        .flex_col()
        .gap(px(14.))
        .child(widgets::title(tr("ui.activity_view.tracking"), 15.));
    if !event.organizer.is_empty() {
        column = column
            .child(person(
                this,
                &event.organizer,
                &event.organizer,
                tr("ui.activity_view.organizer"),
                theme.muted,
            ))
            .child(widgets::divider(&theme));
    }
    for attendee in &event.form.attendees {
        let name = if attendee.name.is_empty() {
            &attendee.address
        } else {
            &attendee.name
        };
        let (status, color) = attendee_status(&attendee.response, &theme);
        let status = if attendee.optional {
            status + &tr("ui.activity_view.optional")
        } else {
            status
        };
        column = column.child(person(this, name, &attendee.address, status, color));
    }
    div()
        .id("meeting-attendees")
        .w(px(280.))
        .flex_none()
        .h_full()
        .overflow_y_scroll()
        .child(column)
        .into_any_element()
}

fn person(this: &mut Oxide, name: &str, id: &str, status: String, color: Hsla) -> AnyElement {
    let avatar = this.avatar(id, name, 30., AvatarShape::Round, None);
    div()
        .flex()
        .items_center()
        .gap(px(10.))
        .child(avatar)
        .child(
            div()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(1.))
                .child(
                    div()
                        .line_clamp(1)
                        .text_ellipsis()
                        .text_size(px(13.))
                        .child(name.to_owned()),
                )
                .child(div().text_size(px(11.)).text_color(color).child(status)),
        )
        .into_any_element()
}

fn response_label(value: &str) -> String {
    let key = match value {
        "Accepted" => "accepted",
        "Declined" => "declined",
        "Tentative" | "TentativelyAccepted" => "tentative",
        "Organizer" => "organizer",
        "NotResponded" | "None" => "none",
        other => return other.to_owned(),
    };
    tr(&format!("ui.activity_view.response.{key}"))
}

fn attendee_status(response: &str, theme: &Theme) -> (String, Hsla) {
    match response {
        "Accepted" => (tr("ui.activity_view.accepted"), green(theme)),
        "Declined" => (tr("ui.activity_view.declined"), theme.danger),
        "Tentative" | "TentativelyAccepted" => (tr("ui.activity_view.tentative"), orange(theme)),
        "Organizer" => (tr("ui.activity_view.organizer"), theme.muted),
        _ => (tr("ui.activity_view.no_response"), theme.muted),
    }
}

/// "3 nov., 09:00 - 10:00" from the event's `YYYY-MM-DDTHH:MM` start and end, when the notification has no context.
fn range(event: &CalendarEventView) -> String {
    let parse = |s: &str| -> Option<(u32, u32, u32, u32)> {
        let month = s.get(5..7)?.parse().ok()?;
        let day = s.get(8..10)?.parse().ok()?;
        let hour = s.get(11..13).and_then(|h| h.parse().ok()).unwrap_or(0);
        let minute = s.get(14..16).and_then(|m| m.parse().ok()).unwrap_or(0);
        Some((month, day, hour, minute))
    };
    let Some((month, day, hour, minute)) = parse(&event.form.start) else {
        return String::new();
    };
    const FR: [&str; 12] = [
        "janv.", "févr.", "mars", "avr.", "mai", "juin", "juil.", "août", "sept.", "oct.", "nov.",
        "déc.",
    ];
    const EN: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let m = (month.clamp(1, 12) - 1) as usize;
    let date = if english() {
        format!("{} {day}", EN[m])
    } else {
        format!("{day} {}", FR[m])
    };
    if event.form.all_day {
        return date;
    }
    let time = |h: u32, min: u32| {
        if english() {
            format!(
                "{}:{min:02} {}",
                if h % 12 == 0 { 12 } else { h % 12 },
                if h < 12 { "AM" } else { "PM" }
            )
        } else {
            format!("{h:02}:{min:02}")
        }
    };
    match parse(&event.form.end) {
        Some((_, _, h, min)) => format!("{date}, {} - {}", time(hour, minute), time(h, min)),
        None => format!("{date}, {}", time(hour, minute)),
    }
}

/// Dialogs and floating panels of this module, drawn over the window.
pub fn overlay(
    _this: &mut Oxide,
    _window: &mut Window,
    _cx: &mut Context<Oxide>,
) -> Option<AnyElement> {
    None
}

/// A step of `OXIDE_SCRIPT` this module understands: `activity:open <n>` (n-th notification), `activity:more`,
/// `activity:rsvp` (opens the answer menu of a meeting's details).
pub fn debug_step(
    this: &mut Oxide,
    step: &str,
    _window: &mut Window,
    cx: &mut Context<Oxide>,
) -> bool {
    let Some(rest) = step.strip_prefix("activity:") else {
        return false;
    };
    let (verb, arg) = rest.split_once(' ').unwrap_or((rest, ""));
    match verb {
        "open" => {
            let Some(row) = arg
                .parse::<usize>()
                .ok()
                .and_then(|n| this.sidebar.as_ref()?.activities.get(n).cloned())
            else {
                return false;
            };
            this.send(Action::OpenActivity { id: row.id });
        }
        "more" => this.send(Action::MoreActivity),
        "rsvp" => this.toggle_popup(RSVP, cx),
        _ => return false,
    }
    true
}

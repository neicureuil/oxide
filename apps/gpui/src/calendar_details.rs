//! The selected event beside the calendar (`CalendarDetails.swift`): answers, edit, cancel or delete, and the attendees.
use crate::calendar::{self, Destructive, send};
use crate::calendar_dates::{self as dates, Day, parse_iso};
use crate::calendar_model::{Status, title};
use crate::calendar_parts::{attendee_badge, primary_icon_button, segmented};
use crate::icons::{Icon, icon};
use crate::oxide::{Oxide, tr, trf};
use crate::theme::Theme;
use crate::widgets::{self, ButtonKind};
use gpui::{AnyElement, Context, FontWeight, SharedString, div, prelude::*, px};
use teams_core::{Action, CalendarAction, CalendarEventView, CalendarPerson, CalendarView};

pub fn panel(
    this: &Oxide,
    event: &CalendarEventView,
    model: &CalendarView,
    theme: &Theme,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    let locked = model.busy || model.uncertain;
    let whole_series = this.ui.calendar.whole_series;
    let mut body = div().p(px(20.)).flex().flex_col().gap_4().w_full();

    body = body.child(
        div()
            .flex()
            .items_center()
            .child(
                div()
                    .flex_1()
                    .text_size(px(15.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(tr("ui.calendar_screen.details")),
            )
            .child(widgets::disabled(
                widgets::icon_button_sized("cal-detail-close", Icon::Close, 26., 13., false, theme)
                    .when(!model.busy, |d| {
                        d.on_click(
                            cx.listener(|this, _, _, _| send(this, CalendarAction::CloseDetail)),
                        )
                    }),
                model.busy,
            )),
    );
    body = body.child(
        div()
            .text_size(px(21.))
            .line_height(px(27.))
            .font_weight(FontWeight::SEMIBOLD)
            .child(title(event)),
    );
    if event.cancelled {
        body = body.child(
            div()
                .text_color(theme.danger)
                .child(tr("ui.calendar_screen.meeting_canceled")),
        );
    }
    if let (Some(start), Some(end)) = (parse_iso(&event.form.start), parse_iso(&event.form.end)) {
        body = body.child(
            div()
                .text_size(px(13.))
                .text_color(theme.text_soft)
                .child(when(start, end, event.form.all_day)),
        );
    }
    body = body.child(widgets::muted(
        trf(
            "ui.calendar_screen.time_zone",
            &[("identifier", &dates::time_zone_id())],
        ),
        11.,
        theme,
    ));
    if !event.organizer.is_empty() {
        body = body.child(label_row(Icon::Person, event.organizer.clone(), theme));
    }
    if !event.form.location.is_empty() {
        body = body.child(label_row(
            Icon::Location,
            event.form.location.clone(),
            theme,
        ));
    }
    if !event.join_url.is_empty() && !event.cancelled {
        let joined = event.clone();
        let url = event.join_url.clone();
        body = body
            .child(
                div().flex().child(
                    primary_icon_button(
                        "cal-detail-join",
                        Icon::Video,
                        tr("ui.calendar_screen.join_meeting"),
                        theme,
                    )
                    .on_click(cx.listener(move |this, _, _, _| calendar::join(this, &joined))),
                ),
            )
            .child(
                div().flex().child(
                    div()
                        .id("cal-detail-web")
                        .text_size(px(13.))
                        .text_color(theme.accent_text)
                        .cursor_pointer()
                        .hover(|s| s.underline())
                        .on_click(cx.listener(move |this, _, _, _| {
                            this.send(Action::OpenUrl { url: url.clone() })
                        }))
                        .child(tr("ui.calendar_screen.open_in_teams_on_the_web")),
                ),
            );
    }
    if !event.recurrence_text.is_empty() {
        body = body.child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .text_size(px(12.))
                .text_color(theme.text_soft)
                .child(icon(Icon::Repeat, px(13.), theme.text_soft))
                .child(event.recurrence_text.clone()),
        );
    }
    if event.occurrence
        && (event.can_edit || event.can_respond || event.can_cancel || event.can_delete)
    {
        body = body.child(div().flex().child(segmented(
            "cal-series",
            vec![
                (false, tr("ui.calendar_screen.this_event")),
                (true, tr("ui.calendar_screen.the_whole_series")),
            ],
            whole_series,
            false,
            theme,
            cx,
            |this, value, _| this.ui.calendar.whole_series = value,
        )));
    }
    if !event.response.is_empty() {
        body = body.child(div().text_size(px(12.)).child(trf(
            "ui.calendar_screen.your_response",
            &[("response", &response_label(&event.response))],
        )));
    }
    if event.can_respond {
        let status = Status::of(event);
        let answers = [
            (
                "Accept",
                Icon::Check,
                tr("ui.calendar_screen.accept"),
                status == Status::Accepted,
            ),
            (
                "TentativelyAccept",
                Icon::Question,
                tr("ui.calendar_screen.tentative"),
                status == Status::Tentative,
            ),
            (
                "Decline",
                Icon::Close,
                tr("ui.calendar_screen.decline"),
                status == Status::Declined,
            ),
        ];
        let mut row = div()
            .flex()
            .items_center()
            .p(px(2.))
            .rounded(px(9.))
            .bg(theme.input)
            .when(locked, |d| d.opacity(0.45));
        for (i, (value, glyph, label, current)) in answers.into_iter().enumerate() {
            if i > 0 {
                row = row.child(div().w(px(1.)).h(px(18.)).bg(theme.border_strong));
            }
            let fg = if current {
                theme.accent_text
            } else {
                theme.text_soft
            };
            row = row.child(
                div()
                    .id(("cal-answer", i))
                    .flex_1()
                    .min_h(px(28.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .gap_1()
                    .rounded(px(7.))
                    .text_size(px(12.))
                    .whitespace_nowrap()
                    .text_color(fg)
                    .when(current, |d| {
                        d.bg(theme.surface)
                            .font_weight(FontWeight::SEMIBOLD)
                            .shadow_sm()
                    })
                    .when(!locked, |d| {
                        d.cursor_pointer()
                            .on_click(cx.listener(move |this, _, _, _| {
                                let series = this.ui.calendar.whole_series;
                                send(
                                    this,
                                    CalendarAction::Respond {
                                        response: value.to_owned(),
                                        series,
                                    },
                                );
                            }))
                    })
                    .child(icon(glyph, px(12.), fg))
                    .child(label),
            );
        }
        body = body.child(row);
    }
    let mut actions = div().flex().flex_col().items_start().gap_2();
    if event.can_edit {
        let occurrence = event.occurrence;
        actions = actions.child(widgets::disabled(
            widgets::button(
                "cal-edit",
                tr("ui.calendar_screen.edit"),
                ButtonKind::Outline,
                theme,
            )
            .h(px(28.))
            .when(!locked, |d| {
                d.on_click(cx.listener(move |this, _, _, _| {
                    let series = occurrence && this.ui.calendar.whole_series;
                    send(this, CalendarAction::Edit { series });
                }))
            }),
            locked,
        ));
    }
    if event.can_cancel {
        actions = actions.child(destructive_button(
            "cal-cancel",
            tr("ui.calendar_screen.cancel_meeting"),
            Destructive::Cancel,
            locked,
            theme,
            cx,
        ));
    }
    if event.can_delete {
        actions = actions.child(destructive_button(
            "cal-delete",
            tr("ui.calendar_screen.delete_from_my_calendar"),
            Destructive::Delete,
            locked,
            theme,
            cx,
        ));
    }
    body = body.child(actions);
    if model.busy {
        body = body.child(widgets::spinner("cal-detail-busy", 14., theme.muted));
    }
    if !event.form.description.is_empty() {
        body = body.child(widgets::divider(theme)).child(
            div()
                .text_size(px(13.))
                .line_height(px(19.))
                .child(event.form.description.clone()),
        );
    }
    if !event.form.attendees.is_empty() {
        body = body
            .child(widgets::divider(theme))
            .child(
                div()
                    .text_size(px(13.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(tr("ui.calendar_screen.attendees")),
            )
            .children(
                event
                    .form
                    .attendees
                    .iter()
                    .enumerate()
                    .map(|(i, p)| attendee_row(i, p, theme)),
            );
    }
    div()
        .id("cal-detail")
        .w(px(330.))
        .flex_none()
        .h_full()
        .overflow_y_scroll()
        .bg(theme.surface)
        .child(body)
        .into_any_element()
}

fn label_row(glyph: Icon, text: String, theme: &Theme) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap_2()
        .child(icon(glyph, px(14.), theme.text_soft))
        .child(div().flex_1().min_w_0().child(text))
        .into_any_element()
}

/// A bordered button in the danger colour (`role: .destructive`), confirmed by the calendar screen.
fn destructive_button(
    id: &'static str,
    label: String,
    kind: Destructive,
    locked: bool,
    theme: &Theme,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    widgets::disabled(
        widgets::button(id, label, ButtonKind::Outline, theme)
            .h(px(28.))
            .text_color(theme.danger)
            .when(!locked, |d| {
                d.on_click(cx.listener(move |this, _, _, cx| {
                    this.ui.calendar.destructive = Some(kind);
                    cx.notify();
                }))
            }),
        locked,
    )
    .into_any_element()
}

/// "jeudi 20 novembre · 09:00 – 10:00", or the days of an all-day or multi-day event.
fn when(start: i64, end: i64, all_day: bool) -> String {
    let first = Day::of(start);
    if all_day {
        let last = Day::of(end).add(-1).max(first);
        return if first == last {
            dates::long_day(first) + &tr("ui.calendar_screen.all_day")
        } else {
            format!("{} → {}", dates::long_day(first), dates::long_day(last))
        };
    }
    if first == Day::of(end) {
        return format!(
            "{} · {} – {}",
            dates::long_day(first),
            dates::time(start),
            dates::time(end)
        );
    }
    format!("{} → {}", dates::date_time(start), dates::date_time(end))
}

fn attendee_row(i: usize, person: &CalendarPerson, theme: &Theme) -> AnyElement {
    let answer = match person.response.as_str() {
        "Accepted" => tr("ui.calendar_screen.accepted"),
        "TentativelyAccepted" | "Tentative" => tr("ui.calendar_screen.tentative"),
        "Declined" => tr("ui.calendar_screen.declined"),
        "Organizer" => tr("ui.calendar_screen.organizer"),
        _ => String::new(),
    };
    let info: Vec<String> = [
        if person.optional {
            tr("ui.calendar_screen.optional")
        } else {
            String::new()
        },
        answer,
    ]
    .into_iter()
    .filter(|s| !s.is_empty())
    .collect();
    let info = if info.is_empty() {
        person.address.clone()
    } else {
        info.join(" · ")
    };
    div()
        .id(("cal-attendee", i))
        .flex()
        .items_center()
        .gap(px(10.))
        .tooltip(widgets::tooltip(
            SharedString::from(person.address.clone()),
            *theme,
        ))
        .child(attendee_badge(person))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(1.))
                .child(
                    div()
                        .text_size(px(13.))
                        .truncate()
                        .child(if person.name.is_empty() {
                            person.address.clone()
                        } else {
                            person.name.clone()
                        }),
                )
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(theme.muted)
                        .truncate()
                        .child(info),
                ),
        )
        .into_any_element()
}

fn response_label(value: &str) -> String {
    match value {
        "Accepted" => tr("ui.calendar_screen.accepted_2"),
        "Declined" => tr("ui.calendar_screen.declined_2"),
        "Tentative" => tr("ui.calendar_screen.tentative"),
        "Organizer" => tr("ui.calendar_screen.organizer"),
        "NotResponded" => tr("ui.calendar_screen.not_responded"),
        "None" => tr("ui.calendar_screen.none"),
        other => other.to_owned(),
    }
}

//! Pieces of the calendar screen (`CalendarParts.swift`): event chips, status marks, the rail's small month,
//! next meeting and legend, and the agenda list.
use crate::calendar::{self, Mode};
use crate::calendar_dates::{self as dates, Day};
use crate::calendar_model::{Entry, Index, Status, title};
use crate::icons::{Icon, icon};
use crate::oxide::{Oxide, tr, trf, trp};
use crate::theme::Theme;
use crate::widgets::{self, ButtonKind};
use gpui::{
    AnyElement, Context, Div, ElementId, FontWeight, Hsla, ScrollHandle, SharedString, Stateful,
    Window, div, prelude::*, px,
};
use std::rc::Rc;
use teams_core::{CalendarEventView, CalendarView};

/// What a click on a day does, in the small month grids (rail and editor).
pub type Pick = Rc<dyn Fn(&mut Oxide, Day, &mut Window, &mut Context<Oxide>)>;

/// Colour bar shared by the timeline blocks, the agenda rows and the legend.
pub fn status_mark(status: Status, width: f32, theme: &Theme) -> Div {
    let d = div().w(px(width)).flex_none().rounded(px(width / 2.));
    match status {
        Status::Accepted => d.bg(theme.accent),
        Status::Tentative => d.border_1().border_dashed().border_color(theme.accent),
        Status::Pending => d.border_1().border_color(theme.accent),
        Status::Declined | Status::Cancelled => d.bg(theme.border_strong),
    }
}

/// Background, outline and text colour of an event block for its answer; `selected` rings it with the accent.
pub fn event_chip(
    d: Stateful<Div>,
    status: Status,
    selected: bool,
    theme: &Theme,
) -> Stateful<Div> {
    let d = d
        .relative()
        .overflow_hidden()
        .rounded(px(5.))
        .text_color(if status.faded() {
            theme.muted
        } else {
            theme.accent_text
        })
        .bg(match status {
            Status::Accepted => theme.accent_soft,
            Status::Tentative => theme.accent_soft.opacity(0.5),
            Status::Pending => theme.surface,
            Status::Declined | Status::Cancelled => theme.input,
        });
    let d = match status {
        Status::Tentative => d
            .border_1()
            .border_dashed()
            .border_color(theme.accent.opacity(0.7)),
        Status::Pending => d.border_1().border_color(theme.accent),
        _ => d,
    };
    let d = d.when(status == Status::Accepted, |d| {
        d.child(
            div()
                .absolute()
                .left_0()
                .top_0()
                .bottom_0()
                .w(px(3.))
                .bg(theme.accent),
        )
    });
    // Drawn over the content, so the ring does not shift the text.
    d.when(selected, |d| {
        d.child(
            div()
                .absolute()
                .inset_0()
                .rounded(px(5.))
                .border_2()
                .border_color(theme.accent),
        )
    })
}

/// "Annulé · " before a cancelled event's title.
pub fn chip_title(event: &CalendarEventView) -> String {
    let prefix = if event.cancelled {
        tr("ui.calendar_screen.canceled")
    } else {
        String::new()
    };
    prefix + &title(event)
}

/// An event block, as `eventButton`: `fill` stretches it to the height the timeline gives it.
pub fn event_button(
    id: impl Into<ElementId>,
    e: &CalendarEventView,
    start: i64,
    model: &CalendarView,
    fill: bool,
    theme: &Theme,
    cx: &mut Context<Oxide>,
) -> Stateful<Div> {
    let status = Status::of(e);
    let selected = model.selected.as_ref().is_some_and(|s| s.id == e.id);
    let id_value = e.id.clone();
    let busy = model.busy;
    let content = div()
        .flex()
        .flex_col()
        .gap(px(2.))
        .child(
            div()
                .flex()
                .items_start()
                .gap_1()
                .when(e.form.online, |d| {
                    d.child(
                        div()
                            .pt(px(3.))
                            .child(icon(Icon::Video, px(10.), theme.accent_text)),
                    )
                })
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_size(px(12.))
                        .line_height(px(15.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .line_clamp(2)
                        .when(e.cancelled, |d| d.line_through())
                        .child(chip_title(e)),
                ),
        )
        .when(!e.form.all_day, |d| {
            d.child(
                div()
                    .text_size(px(10.))
                    .line_height(px(13.))
                    .child(dates::time(start)),
            )
        });
    let button = div()
        .id(id)
        .w_full()
        .when(fill, |d| d.h_full())
        .pl(px(if status == Status::Accepted { 8. } else { 5. }))
        .pr(px(5.))
        .py(px(4.))
        .cursor_pointer()
        .tooltip(widgets::tooltip(
            format!("{} · {}", title(e), status.label()),
            *theme,
        ))
        .when(!busy, |d| {
            d.on_click(cx.listener(move |this, _, _, _| calendar::select(this, &id_value)))
        })
        .child(content);
    event_chip(button, status, selected, theme)
}

/// All-day and multi-day events: one line under the date, no time.
pub fn banner_button(
    id: impl Into<ElementId>,
    e: &CalendarEventView,
    model: &CalendarView,
    theme: &Theme,
    cx: &mut Context<Oxide>,
) -> Stateful<Div> {
    let status = Status::of(e);
    let selected = model.selected.as_ref().is_some_and(|s| s.id == e.id);
    let id_value = e.id.clone();
    let busy = model.busy;
    let button = div()
        .id(id)
        .w_full()
        .pl(px(if status == Status::Accepted { 7. } else { 5. }))
        .pr(px(5.))
        .py(px(2.))
        .text_size(px(11.))
        .line_height(px(14.))
        .font_weight(FontWeight::SEMIBOLD)
        .cursor_pointer()
        .tooltip(widgets::tooltip(
            format!("{} · {}", title(e), status.label()),
            *theme,
        ))
        .when(!busy, |d| {
            d.on_click(cx.listener(move |this, _, _, _| calendar::select(this, &id_value)))
        })
        .child(
            div()
                .truncate()
                .when(e.cancelled, |d| d.line_through())
                .child(chip_title(e)),
        );
    event_chip(button, status, selected, theme)
}

/// A month of days, Monday first: the rail's navigator and the editor's date fields.
pub struct MonthGrid<'a> {
    pub id: &'static str,
    pub month: Day,
    /// Tinted, as the days the main view shows.
    pub shown: &'a dyn Fn(Day) -> bool,
    /// A dot under the day.
    pub busy: &'a dyn Fn(Day) -> bool,
    pub enabled: &'a dyn Fn(Day) -> bool,
    pub pick: Pick,
}

pub fn month_grid(grid: MonthGrid, theme: &Theme, cx: &mut Context<Oxide>) -> AnyElement {
    let today = Day::today();
    let first = grid.month.month_start().week();
    let mut rows = div()
        .flex()
        .flex_col()
        .gap(px(2.))
        .child(div().flex().children((0..7).map(|i| {
            div()
                .flex_1()
                .h(px(18.))
                .flex()
                .items_center()
                .justify_center()
                .text_size(px(10.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.subtle)
                .child(dates::weekday_narrow(first.add(i).weekday()))
        })));
    for week in 0..6 {
        let mut row = div().flex();
        for i in 0..7 {
            let day = first.add(week * 7 + i);
            let is_today = day == today;
            let in_month = day.same_month(grid.month);
            let enabled = (grid.enabled)(day);
            let pick = grid.pick.clone();
            let fg = if is_today {
                gpui::white()
            } else if in_month {
                theme.text
            } else {
                theme.subtle.opacity(0.6)
            };
            row = row.child(
                div()
                    .id((grid.id, (day.0 as usize)))
                    .flex_1()
                    .min_h(px(26.))
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .gap(px(1.))
                    .when((grid.shown)(day), |d| d.bg(theme.accent_soft))
                    .when(!enabled, |d| d.opacity(0.35))
                    .when(enabled, |d| {
                        d.cursor_pointer().on_click(
                            cx.listener(move |this, _, window, cx| pick(this, day, window, cx)),
                        )
                    })
                    .child(
                        div()
                            .size(px(20.))
                            .rounded_full()
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_size(px(11.))
                            .text_color(fg)
                            .when(is_today, |d| {
                                d.bg(theme.accent).font_weight(FontWeight::BOLD)
                            })
                            .child(day.day().to_string()),
                    )
                    .child(
                        div()
                            .size(px(3.))
                            .rounded_full()
                            .when((grid.busy)(day) && !is_today, |d| {
                                d.bg(theme.accent.opacity(0.6))
                            }),
                    ),
            );
        }
        rows = rows.child(row);
    }
    rows.into_any_element()
}

/// Small month in the rail; its own arrows browse months without moving the main view.
pub fn mini_month(
    this: &Oxide,
    days: &[Day],
    theme: &Theme,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    let ui = &this.ui.calendar;
    let month = ui.rail_month;
    let index = &ui.index;
    let header = div()
        .flex()
        .items_center()
        .gap(px(2.))
        .child(
            div()
                .flex_1()
                .text_size(px(13.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(dates::month_year(month)),
        )
        .child(
            widgets::icon_button_sized("cal-mini-prev", Icon::ChevronLeft, 22., 11., false, theme)
                .tooltip(widgets::tooltip(
                    tr("ui.calendar_parts.previous_month"),
                    *theme,
                ))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.ui.calendar.rail_month = this.ui.calendar.rail_month.add_months(-1);
                    cx.notify();
                })),
        )
        .child(
            widgets::icon_button_sized("cal-mini-next", Icon::ChevronRight, 22., 11., false, theme)
                .tooltip(widgets::tooltip(tr("ui.calendar_parts.next_month"), *theme))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.ui.calendar.rail_month = this.ui.calendar.rail_month.add_months(1);
                    cx.notify();
                })),
        );
    let pick: Pick = Rc::new(|this, day, _, cx| {
        calendar::set_anchor(this, day);
        cx.notify();
    });
    let grid = month_grid(
        MonthGrid {
            id: "cal-mini-day",
            month,
            shown: &|d| days.contains(&d),
            busy: &|d| index.has_events(d),
            enabled: &|_| true,
            pick,
        },
        theme,
        cx,
    );
    div()
        .flex()
        .flex_col()
        .gap(px(6.))
        .child(header)
        .child(grid)
        .into_any_element()
}

/// A primary button with a leading icon (`Label(…, systemImage:)` in `PrimaryButtonStyle`).
pub fn primary_icon_button(
    id: impl Into<ElementId>,
    glyph: Icon,
    label: impl Into<SharedString>,
    theme: &Theme,
) -> Stateful<Div> {
    div()
        .id(id)
        .h(px(32.))
        .px_3()
        .flex()
        .flex_none()
        .items_center()
        .justify_center()
        .gap_1p5()
        .rounded(px(7.))
        .bg(theme.accent)
        .text_color(gpui::white())
        .text_size(px(13.))
        .font_weight(FontWeight::MEDIUM)
        .cursor_pointer()
        .hover(|s| s.opacity(0.9))
        .child(icon(glyph, px(14.), gpui::white()))
        .child(label.into())
}

/// A ghost button with a leading icon (`GhostButtonStyle`).
pub fn ghost_icon_button(
    id: impl Into<ElementId>,
    glyph: Icon,
    label: impl Into<SharedString>,
    accent: bool,
    theme: &Theme,
) -> Stateful<Div> {
    let fg = if accent {
        theme.accent_text
    } else {
        theme.muted
    };
    let hover = theme.input;
    div()
        .id(id)
        .px(px(6.))
        .py(px(4.))
        .flex()
        .items_center()
        .gap_1p5()
        .rounded(px(8.))
        .text_size(px(12.))
        .text_color(fg)
        .cursor_pointer()
        .hover(move |s| s.bg(hover))
        .child(icon(glyph, px(13.), fg))
        .child(label.into())
}

/// The next meeting still to come in the loaded period, with its join button.
pub fn next_meeting(this: &Oxide, theme: &Theme, cx: &mut Context<Oxide>) -> AnyElement {
    let now = dates::now();
    let next = this.ui.calendar.index.all.iter().find(|entry| {
        let status = Status::of(&entry.event);
        !entry.event.form.all_day && entry.end > now && !status.faded()
    });
    let mut card = div()
        .flex()
        .flex_col()
        .gap(px(6.))
        .p(px(10.))
        .rounded(px(10.))
        .border_1()
        .border_color(theme.border)
        .bg(theme.surface);
    match next {
        Some(next) => {
            let id = next.event.id.clone();
            let event = next.event.clone();
            let people = if next.event.form.attendees.is_empty() {
                String::new()
            } else {
                trf(
                    "ui.calendar_parts.people",
                    &[(
                        "attendeesCount",
                        &next.event.form.attendees.len().to_string(),
                    )],
                )
            };
            card = card
                .child(
                    div()
                        .text_size(px(11.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.accent_text)
                        .child(dates::relative(next.start, next.end, now)),
                )
                .child(
                    div()
                        .id("cal-next-title")
                        .text_size(px(13.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.text)
                        .line_clamp(2)
                        .cursor_pointer()
                        .on_click(cx.listener(move |this, _, _, _| calendar::select(this, &id)))
                        .child(title(&next.event)),
                )
                .child(
                    div()
                        .text_size(px(11.))
                        .text_color(theme.muted)
                        .child(format!(
                            "{} – {}{people}",
                            dates::time(next.start),
                            dates::time(next.end)
                        )),
                )
                .when(!next.event.join_url.is_empty(), |d| {
                    d.child(
                        primary_icon_button(
                            "cal-next-join",
                            Icon::Video,
                            tr("ui.calendar_parts.join"),
                            theme,
                        )
                        .w_full()
                        .mt(px(2.))
                        .on_click(cx.listener(move |this, _, _, _| calendar::join(this, &event))),
                    )
                });
        }
        None => {
            card = card
                .child(
                    div()
                        .text_size(px(11.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(theme.muted)
                        .child(tr("ui.calendar_parts.next_meeting")),
                )
                .child(widgets::muted(
                    tr("ui.calendar_parts.nothing_else_in_this_period"),
                    12.,
                    theme,
                ));
        }
    }
    card.into_any_element()
}

pub fn legend(theme: &Theme) -> AnyElement {
    div()
        .flex()
        .flex_col()
        .gap(px(6.))
        .children(
            [Status::Accepted, Status::Tentative, Status::Pending].map(|status| {
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(status_mark(status, 10., theme).h(px(10.)))
                    .child(widgets::muted(status.label(), 11., theme))
            }),
        )
        .into_any_element()
}

/// Mini month, next meeting and legend beside the day and week timelines.
pub fn rail(this: &Oxide, days: &[Day], theme: &Theme, cx: &mut Context<Oxide>) -> AnyElement {
    let pending = this
        .ui
        .calendar
        .index
        .all
        .iter()
        .filter(|e| Status::of(&e.event) == Status::Pending)
        .count();
    div()
        .id("cal-rail")
        .w(px(232.))
        .flex_none()
        .h_full()
        .overflow_y_scroll()
        .bg(theme.background)
        .child(
            div()
                .p(px(14.))
                .flex()
                .flex_col()
                .gap(px(18.))
                .child(mini_month(this, days, theme, cx))
                .child(next_meeting(this, theme, cx))
                .when(pending > 0, |d| {
                    d.child(
                        div().flex().child(
                            ghost_icon_button(
                                "cal-pending",
                                Icon::Envelope,
                                trp("ui.calendar_screen.to_respond_to", pending as i64, &[]),
                                true,
                                theme,
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                calendar::set_mode(this, Mode::Agenda);
                                cx.notify();
                            })),
                        ),
                    )
                })
                .child(legend(theme)),
        )
        .into_any_element()
}

/// A capsule label, as the agenda's "Not responded".
fn capsule(text: String, fg: Hsla, bg: Hsla) -> Div {
    div()
        .flex_none()
        .px_2()
        .py(px(3.))
        .rounded_full()
        .bg(bg)
        .text_size(px(11.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(fg)
        .child(text)
}

/// Events listed day by day, with the answer and join buttons on each row.
pub fn agenda(
    this: &Oxide,
    days: &[Day],
    model: &CalendarView,
    scroll: &ScrollHandle,
    theme: &Theme,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    let now = dates::now();
    let index: &Index = &this.ui.calendar.index;
    let mut list = div()
        .id("cal-agenda")
        .size_full()
        .overflow_y_scroll()
        .track_scroll(scroll);
    for (d_ix, &day) in days.iter().enumerate() {
        let entries = index.entries(day);
        let mut section = div()
            .flex()
            .flex_col()
            .child(agenda_header(day, &entries, theme));
        if entries.is_empty() {
            section = section.child(
                div()
                    .flex()
                    .items_center()
                    .px_4()
                    .py(px(10.))
                    .child(div().flex_1().child(widgets::muted(
                        tr("ui.calendar_parts.nothing_planned"),
                        12.,
                        theme,
                    )))
                    .child(
                        ghost_icon_button(
                            ("cal-agenda-add", d_ix),
                            Icon::Plus,
                            tr("ui.calendar_parts.add"),
                            true,
                            theme,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            calendar::new_event(this, day);
                            cx.notify();
                        })),
                    ),
            );
        } else {
            for (e_ix, entry) in entries.iter().enumerate() {
                section = section
                    .child(agenda_row(entry, day, (d_ix, e_ix), now, model, theme, cx))
                    .child(div().pl(px(86.)).child(widgets::divider(theme)));
            }
        }
        list = list.child(section);
    }
    list.into_any_element()
}

fn agenda_header(day: Day, entries: &[&Entry], theme: &Theme) -> AnyElement {
    let (from, to) = (day.start(), day.add(1).start());
    let meetings: Vec<&&Entry> = entries
        .iter()
        .filter(|e| !e.event.form.all_day && !e.event.cancelled && e.start >= from && e.end <= to)
        .collect();
    let total: i64 = meetings.iter().map(|e| e.end - e.start).sum();
    let today = day == Day::today();
    div()
        .flex()
        .items_center()
        .px_4()
        .py_2()
        .bg(theme.background)
        .border_b_1()
        .border_color(theme.border)
        .child(
            div()
                .flex_1()
                .text_size(px(13.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(if today {
                    theme.accent_text
                } else {
                    theme.text_soft
                })
                .child(dates::day_title(day)),
        )
        .when(!meetings.is_empty(), |d| {
            d.child(widgets::muted(
                trp(
                    "ui.calendar_parts.meetings_total",
                    meetings.len() as i64,
                    &[("total", &dates::duration(total))],
                ),
                11.,
                theme,
            ))
        })
        .into_any_element()
}

fn agenda_row(
    entry: &Entry,
    day: Day,
    id: (usize, usize),
    now: i64,
    model: &CalendarView,
    theme: &Theme,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    let e = &entry.event;
    let status = Status::of(e);
    let full_day = entry.is_banner(day);
    let past = entry.end <= now;
    let soon = !full_day && !past && entry.start - now < 15 * 60;
    let selected = model.selected.as_ref().is_some_and(|s| s.id == e.id);
    let details: Vec<String> = [
        e.form.location.clone(),
        if e.form.online {
            tr("ui.calendar_parts.teams_meeting")
        } else {
            String::new()
        },
        if e.form.attendees.is_empty() {
            String::new()
        } else {
            trf(
                "ui.calendar_parts.attendees",
                &[("attendeesCount", &e.form.attendees.len().to_string())],
            )
        },
    ]
    .into_iter()
    .filter(|s| !s.is_empty())
    .collect();
    let details = details.join(" · ");
    let id_value = e.id.clone();
    let event = e.clone();
    let row_id = SharedString::from(format!("cal-agenda-{}-{}", id.0, id.1));
    let trailing: Option<AnyElement> =
        if soon && !e.join_url.is_empty() && status != Status::Cancelled {
            Some(
                primary_icon_button(
                    SharedString::from(format!("{row_id}-join")),
                    Icon::Video,
                    tr("ui.calendar_parts.join"),
                    theme,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    cx.stop_propagation();
                    calendar::join(this, &event)
                }))
                .into_any_element(),
            )
        } else if matches!(status, Status::Pending | Status::Cancelled) {
            let cancelled = status == Status::Cancelled;
            Some(
                capsule(
                    status.label(),
                    if cancelled {
                        theme.danger
                    } else {
                        theme.accent_text
                    },
                    if cancelled {
                        theme.error_background
                    } else {
                        theme.accent_soft
                    },
                )
                .into_any_element(),
            )
        } else {
            None
        };
    div()
        .id(row_id.clone())
        .flex()
        .items_center()
        .gap_3()
        .px_4()
        .py(px(9.))
        .cursor_pointer()
        .when(selected, |d| d.bg(theme.accent_soft))
        .when(!selected && soon, |d| d.bg(theme.accent_soft.opacity(0.4)))
        .tooltip(widgets::tooltip(title(e), *theme))
        .when(!model.busy, |d| {
            d.on_click(cx.listener(move |this, _, _, _| calendar::select(this, &id_value)))
        })
        .child(
            div()
                .flex()
                .flex_1()
                .min_w_0()
                .items_center()
                .gap_3()
                .when(past && !selected, |d| d.opacity(0.55))
                .child(
                    div()
                        .w(px(58.))
                        .flex_none()
                        .flex()
                        .flex_col()
                        .items_end()
                        .gap(px(2.))
                        .child(
                            div()
                                .text_size(px(12.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme.text_soft)
                                .child(if full_day {
                                    tr("ui.calendar_parts.all_day")
                                } else {
                                    dates::time(entry.start)
                                }),
                        )
                        .when(!full_day, |d| {
                            d.child(widgets::muted(
                                dates::duration(entry.end - entry.start),
                                11.,
                                theme,
                            ))
                        }),
                )
                .child({
                    // Stretched to the row's height.
                    let mut mark = status_mark(status, 4., theme).min_h(px(30.));
                    mark.style().align_self = Some(gpui::AlignSelf::Stretch);
                    mark
                })
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap(px(3.))
                        .child(
                            div()
                                .text_size(px(13.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(if status.faded() {
                                    theme.muted
                                } else {
                                    theme.text
                                })
                                .line_clamp(2)
                                .when(status == Status::Cancelled, |d| d.line_through())
                                .child(title(e)),
                        )
                        .map(|d| {
                            if soon {
                                d.child(
                                    div()
                                        .text_size(px(11.))
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(theme.accent_text)
                                        .child(dates::relative(entry.start, entry.end, now)),
                                )
                            } else if !details.is_empty() {
                                d.child(
                                    div()
                                        .text_size(px(11.))
                                        .text_color(theme.muted)
                                        .truncate()
                                        .child(details),
                                )
                            } else {
                                d
                            }
                        }),
                )
                .children(trailing),
        )
        .into_any_element()
}

/// The app's segmented control (`SegmentedPicker`).
pub fn segmented<T: Copy + PartialEq + 'static>(
    id: &'static str,
    options: Vec<(T, String)>,
    selected: T,
    disabled: bool,
    theme: &Theme,
    cx: &mut Context<Oxide>,
    on: impl Fn(&mut Oxide, T, &mut Context<Oxide>) + 'static,
) -> Div {
    let on = Rc::new(on);
    // The chosen segment's tint slides to the new one (`withAnimation(Motion.select)`).
    let accent = theme.accent_soft;
    let (highlight, slid, measure) = crate::motion::sliding_selection(
        &format!("segmented-{id}"),
        options.iter().position(|(v, _)| *v == selected),
        3.,
        move |d| d.top(px(3.)).bottom(px(3.)).rounded(px(6.)).bg(accent),
    );
    div()
        .relative()
        .flex()
        .flex_none()
        .gap(px(2.))
        .p(px(3.))
        .rounded(px(8.))
        .bg(theme.surface)
        .border_1()
        .border_color(theme.border_strong)
        .when(disabled, |d| d.opacity(0.45))
        .on_children_prepainted(measure)
        .child(highlight.unwrap_or_else(|| div().into_any_element()))
        .children(options.into_iter().enumerate().map(|(i, (value, label))| {
            let active = value == selected;
            let on = on.clone();
            div()
                .id((id, i))
                .min_h(px(26.))
                .px(px(14.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(6.))
                .text_size(px(13.))
                .text_color(if active {
                    theme.accent_text
                } else {
                    theme.text_soft
                })
                .when(active, |d| {
                    d.when(!slid, |d| d.bg(theme.accent_soft))
                        .font_weight(FontWeight::SEMIBOLD)
                })
                .when(!disabled, |d| {
                    d.cursor_pointer()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            on(this, value, cx);
                            cx.notify();
                        }))
                })
                .child(label)
        }))
}

/// A bordered button with an icon only, as SwiftUI's default buttons in the toolbar.
pub fn outline_icon_button(id: impl Into<ElementId>, glyph: Icon, theme: &Theme) -> Stateful<Div> {
    let hover = theme.input;
    div()
        .id(id)
        .size(px(28.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(7.))
        .border_1()
        .border_color(theme.border_strong)
        .bg(theme.surface)
        .cursor_pointer()
        .hover(move |s| s.bg(hover))
        .child(icon(glyph, px(13.), theme.text_soft))
}

/// A bordered text button, smaller than `widgets::button`'s outline kind.
pub fn small_button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    theme: &Theme,
) -> Stateful<Div> {
    widgets::button(id, label, ButtonKind::Outline, theme).h(px(28.))
}

/// Initials in a coloured circle; attendees only come with an address, not an id to fetch a photo.
pub fn attendee_badge(person: &teams_core::CalendarPerson) -> AnyElement {
    let name = if person.name.is_empty() {
        &person.address
    } else {
        &person.name
    };
    let words: Vec<&str> = name
        .split(|c: char| c.is_whitespace() || c == '.' || c == '@')
        .filter(|w| !w.is_empty())
        .collect();
    let initials: String = if words.len() > 1 {
        words[0]
            .chars()
            .take(1)
            .chain(words[1].chars().take(1))
            .collect()
    } else {
        name.chars().take(2).collect()
    };
    div()
        .size(px(26.))
        .flex_none()
        .rounded_full()
        .bg(Theme::avatar(&person.address.to_lowercase()))
        .flex()
        .items_center()
        .justify_center()
        .text_size(px(10.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(gpui::white())
        .child(initials.to_uppercase())
        .into_any_element()
}

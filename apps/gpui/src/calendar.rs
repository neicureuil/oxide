//! The calendar screen (`CalendarScreen.swift`): agenda, day, week and month views with their own navigation,
//! the selected event beside them, and the event editor and confirmations over the window.
use crate::calendar_dates::{self as dates, Day};
use crate::calendar_editor::{self, Editor};
use crate::calendar_model::{self as model_, DAY_HEIGHT, HOUR, Index, layout};
use crate::calendar_parts::{self as parts, outline_icon_button, primary_icon_button, segmented};
use crate::icons::{Icon, icon};
use crate::oxide::{Oxide, text, tr};
use crate::theme::Theme;
use crate::widgets::{self, ButtonKind};
use gpui::{
    AnyElement, Context, FocusHandle, FontWeight, Hsla, KeyDownEvent, ScrollHandle, SharedString,
    Task, Window, div, point, prelude::*, px,
};
use std::time::Duration;
use teams_core::{Action, CalendarAction, CalendarEventView, CalendarView, CallAction, Nav};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    Agenda,
    Day,
    Week,
    Month,
}

impl Mode {
    fn pref(self) -> &'static str {
        match self {
            Mode::Agenda => "agenda",
            Mode::Day => "day",
            Mode::Week => "week",
            Mode::Month => "month",
        }
    }
    fn from_pref(value: &str) -> Self {
        Self::ALL.into_iter().find(|m| m.pref() == value).unwrap_or(Mode::Week)
    }
    const ALL: [Mode; 4] = [Mode::Agenda, Mode::Day, Mode::Week, Mode::Month];
    fn label(self) -> String {
        tr(match self {
            Mode::Agenda => "ui.calendar_screen.agenda",
            Mode::Day => "ui.calendar_screen.day",
            Mode::Week => "ui.calendar_screen.week",
            Mode::Month => "ui.calendar_screen.month",
        })
    }
}

/// A confirmation asked before cancelling or deleting the selected event.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Destructive {
    Cancel,
    Delete,
}

pub struct Ui {
    pub mode: Mode,
    /// The day the views are built around.
    pub anchor: Day,
    pub destructive: Option<Destructive>,
    /// For one date of a series: whether edits and answers go to the whole series.
    pub whole_series: bool,
    /// The core's events by day; emptied while the screen is hidden.
    pub index: Index,
    /// The month the rail's small month shows, and the anchor it last followed.
    pub rail_month: Day,
    rail_anchor: Day,
    pub editor: Editor,
    /// Drawn on the last frame: the next appearance reloads the period and scrolls to the morning.
    visible: bool,
    sent_range: Option<(String, String)>,
    /// Scroll the day grid to 8:00, or the agenda to today, on the next frame.
    scroll_pending: bool,
    timeline_scroll: ScrollHandle,
    agenda_scroll: ScrollHandle,
    focus: FocusHandle,
    /// Redraws every minute while shown, for the current-time line and "in 5 min".
    ticker: Option<Task<()>>,
}

impl Ui {
    pub fn new(window: &mut Window, cx: &mut Context<Oxide>) -> Self {
        let today = Day::today();
        Self {
            mode: Mode::from_pref(&crate::prefs::Prefs::load().calendar_mode),
            anchor: today,
            destructive: None,
            whole_series: false,
            index: Index::default(),
            rail_month: today,
            rail_anchor: today,
            editor: Editor::new(window, cx),
            visible: false,
            sent_range: None,
            scroll_pending: true,
            timeline_scroll: ScrollHandle::new(),
            agenda_scroll: ScrollHandle::new(),
            focus: cx.focus_handle(),
            ticker: None,
        }
    }
}

pub fn send(this: &Oxide, action: CalendarAction) {
    this.send(Action::Calendar { action });
}

pub fn select(this: &mut Oxide, id: &str) {
    send(this, CalendarAction::Select { id: id.to_owned() });
}

pub fn join(this: &mut Oxide, event: &CalendarEventView) {
    this.send(Action::Call {
        action: CallAction::PrepareMeeting {
            url: event.join_url.clone(),
            title: model_::title(event),
        },
    });
}

/// A new event at 9:00 on `day`.
pub fn new_event(this: &mut Oxide, day: Day) {
    create_event(this, day.at(9, 0));
}

pub fn create_event(this: &mut Oxide, start: i64) {
    if this
        .calendar
        .as_ref()
        .is_some_and(|c| c.busy || c.draft.is_some())
    {
        return;
    }
    send(
        this,
        CalendarAction::New {
            start: dates::iso(start),
            end: dates::iso(start + 30 * 60),
            time_zone: dates::time_zone_id(),
        },
    );
}

pub fn set_anchor(this: &mut Oxide, day: Day) {
    this.ui.calendar.anchor = day;
}

pub fn set_mode(this: &mut Oxide, mode: Mode) {
    let ui = &mut this.ui.calendar;
    if ui.mode != mode {
        ui.mode = mode;
        ui.scroll_pending = true;
        // Kept between launches, as Swift's `@AppStorage`.
        this.prefs.calendar_mode = mode.pref().to_owned();
        this.prefs.save();
    }
}

/// The previous or next period.
fn shift(this: &mut Oxide, amount: i32) {
    let ui = &mut this.ui.calendar;
    ui.anchor = match ui.mode {
        Mode::Month => ui.anchor.add_months(amount),
        Mode::Week | Mode::Agenda => ui.anchor.add(amount * 7),
        Mode::Day => ui.anchor.add(amount),
    };
}

/// The days the current view shows.
fn days(ui: &Ui) -> Vec<Day> {
    let (start, count) = match ui.mode {
        Mode::Agenda => (ui.anchor, 7),
        Mode::Day => (ui.anchor, 1),
        Mode::Week => (ui.anchor.week(), 7),
        Mode::Month => (ui.anchor.month_start().week(), 42),
    };
    (0..count).map(|i| start.add(i)).collect()
}

/// "16 – 22 nov. 2026", "jeudi 20 novembre 2026" or "novembre 2026".
fn period(ui: &Ui, days: &[Day]) -> String {
    match ui.mode {
        Mode::Day => dates::long_day_year(ui.anchor),
        Mode::Month => dates::month_year(ui.anchor),
        Mode::Agenda | Mode::Week => {
            let (first, last) = (days[0], days[days.len() - 1]);
            let from = if first.same_month(last) {
                first.day().to_string()
            } else {
                dates::day_month_short(first)
            };
            format!("{from} – {}", dates::day_month_short_year(last))
        }
    }
}

/// Shown as the screen: not when search or a new chat covers it.
fn showing(this: &Oxide) -> bool {
    this.shell
        .as_ref()
        .is_some_and(|s| s.signed_in && s.nav == Nav::Calendar)
        && !this.search.as_ref().is_some_and(|s| s.open)
        && this.new_chat.is_none()
}

/// What `onAppear` and the change handlers do in Swift, once per frame.
fn prepare(this: &mut Oxide, window: &mut Window, cx: &mut Context<Oxide>) {
    let ui = &mut this.ui.calendar;
    if !ui.visible {
        ui.visible = true;
        ui.sent_range = None;
        ui.scroll_pending = true;
        ui.ticker = Some(cx.spawn(async move |this, cx| {
            loop {
                let wait = 60 - dates::now().rem_euclid(60) as u64;
                cx.background_executor()
                    .timer(Duration::from_secs(wait))
                    .await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        }));
        if window.focused(cx).is_none() {
            window.focus(&ui.focus);
        }
    }
    if ui.rail_anchor != ui.anchor {
        ui.rail_anchor = ui.anchor;
        ui.rail_month = ui.anchor;
    }
    let events: &[CalendarEventView] = this
        .calendar
        .as_ref()
        .map(|c| c.events.as_slice())
        .unwrap_or(&[]);
    if this.ui.calendar.index.key != model_::key(events) {
        this.ui.calendar.index = Index::new(events);
    }
    let days = days(&this.ui.calendar);
    let range = (
        dates::iso(days[0].start()),
        dates::iso(days[days.len() - 1].add(1).start()),
    );
    if this.ui.calendar.sent_range.as_ref() != Some(&range) {
        send(
            this,
            CalendarAction::Range {
                start: range.0.clone(),
                end: range.1.clone(),
            },
        );
        this.ui.calendar.sent_range = Some(range);
    }
}

/// Hidden: drop what only the screen needs.
fn hide(this: &mut Oxide) {
    let ui = &mut this.ui.calendar;
    if ui.visible {
        ui.visible = false;
        ui.index = Index::default();
        ui.ticker = None;
    }
}

pub fn screen(this: &mut Oxide, window: &mut Window, cx: &mut Context<Oxide>) -> AnyElement {
    prepare(this, window, cx);
    // Lent to the drawing rather than copied each frame; nothing drawn reads `this.calendar`.
    let taken = this.calendar.take();
    let default = CalendarView::default();
    let element = draw(this, taken.as_ref().unwrap_or(&default), window, cx);
    this.calendar = taken;
    element
}

fn draw(
    this: &mut Oxide,
    model: &CalendarView,
    window: &mut Window,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    let theme = this.theme;
    let days = days(&this.ui.calendar);
    let mode = this.ui.calendar.mode;

    let main_view = match mode {
        Mode::Agenda => {
            let scroll = this.ui.calendar.agenda_scroll.clone();
            if this.ui.calendar.scroll_pending {
                this.ui.calendar.scroll_pending = false;
                if let Some(i) = days.iter().position(|&d| d == Day::today()) {
                    scroll.scroll_to_item(i);
                }
            }
            parts::agenda(this, &days, model, &scroll, &theme, cx)
        }
        Mode::Month => month_grid(this, &days, model, &theme, cx),
        Mode::Day | Mode::Week => timeline(this, &days, model, &theme, window, cx),
    };
    let main_area = div()
        .flex_1()
        .min_w_0()
        .h_full()
        .flex()
        .flex_col()
        .when(model.loading, |d| {
            d.child(
                div()
                    .flex()
                    .justify_center()
                    .p(px(6.))
                    .child(widgets::spinner("cal-loading", 14., theme.muted)),
            )
        })
        .child(div().flex_1().min_h_0().child(main_view))
        .when(
            mode != Mode::Agenda && !model.loading && model.events.is_empty(),
            |d| {
                d.child(div().flex().justify_center().p_2().child(widgets::muted(
                    tr("ui.calendar_screen.no_events_in_this_period"),
                    12.,
                    &theme,
                )))
            },
        );

    let side: Option<AnyElement> = if model.loading_detail {
        Some(
            div()
                .w(px(330.))
                .flex_none()
                .h(px(200.))
                .flex()
                .items_center()
                .justify_center()
                .child(widgets::loading(
                    "cal-detail-loading",
                    tr("ui.calendar_screen.loading_event"),
                    &theme,
                ))
                .into_any_element(),
        )
    } else if let Some(event) = &model.selected {
        Some(crate::calendar_details::panel(
            this, event, model, &theme, cx,
        ))
    } else if mode == Mode::Agenda {
        Some(
            div()
                .w(px(330.))
                .flex_none()
                .h_full()
                .p(px(24.))
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap(px(10.))
                .child(icon(Icon::Calendar, px(28.), theme.subtle))
                .child(
                    div()
                        .text_size(px(12.))
                        .text_color(theme.muted)
                        .text_center()
                        .child(tr("ui.calendar_screen.select_an_event_to_see_its")),
                )
                .into_any_element(),
        )
    } else {
        None
    };

    let rail = matches!(mode, Mode::Day | Mode::Week).then(|| parts::rail(this, &days, &theme, cx));
    let vline = |theme: &Theme| div().w(px(1.)).h_full().flex_none().bg(theme.border);
    let focus = this.ui.calendar.focus.clone();
    div()
        .id("calendar")
        .size_full()
        .flex()
        .flex_col()
        .bg(theme.surface)
        .track_focus(&focus)
        // ← and → change the period and T comes back to today, while no field has the focus.
        .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
            let keys = &event.keystroke;
            let m = keys.modifiers;
            if m.control || m.alt || m.platform || m.function {
                return;
            }
            if this.ui.calendar.destructive.is_some()
                || this.calendar.as_ref().is_some_and(|c| c.draft.is_some())
            {
                return;
            }
            match keys.key.as_str() {
                "left" => shift(this, -1),
                "right" => shift(this, 1),
                "t" => set_anchor(this, Day::today()),
                _ => return,
            }
            cx.stop_propagation();
            cx.notify();
        }))
        .on_mouse_down(gpui::MouseButton::Left, move |_, window, _| {
            // Clicking the calendar takes the focus back for the arrow keys.
            if !focus.is_focused(window) {
                window.focus(&focus);
            }
        })
        .child(toolbar(this, &days, model, &theme, cx))
        .when_some(
            model.error.clone().filter(|_| model.draft.is_none()),
            |d, error| d.child(error_banner(error, model.busy, &theme, cx)),
        )
        .child(widgets::divider(&theme))
        .child(
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .when_some(rail, |d, rail| d.child(rail).child(vline(&theme)))
                .child(main_area)
                .when_some(side, |d, side| d.child(vline(&theme)).child(side)),
        )
        .into_any_element()
}

fn toolbar(
    this: &Oxide,
    days: &[Day],
    model: &CalendarView,
    theme: &Theme,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    let ui = &this.ui.calendar;
    let creating_blocked = model.busy || model.draft.is_some();
    div()
        .flex()
        .items_center()
        .gap_3()
        .p_4()
        .child(
            div()
                .text_size(px(22.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(tr("ui.calendar_screen.calendar")),
        )
        .child(
            parts::small_button("cal-today", tr("ui.calendar_screen.today"), theme)
                .tooltip(widgets::tooltip(tr("ui.calendar_screen.today_t"), *theme))
                .on_click(cx.listener(|this, _, _, cx| {
                    set_anchor(this, Day::today());
                    cx.notify();
                })),
        )
        .child(
            outline_icon_button("cal-prev", Icon::ChevronLeft, theme)
                .tooltip(widgets::tooltip(
                    tr("ui.calendar_screen.previous_period_2"),
                    *theme,
                ))
                .on_click(cx.listener(|this, _, _, cx| {
                    shift(this, -1);
                    cx.notify();
                })),
        )
        .child(
            outline_icon_button("cal-next", Icon::ChevronRight, theme)
                .tooltip(widgets::tooltip(
                    tr("ui.calendar_screen.next_period_2"),
                    *theme,
                ))
                .on_click(cx.listener(|this, _, _, cx| {
                    shift(this, 1);
                    cx.notify();
                })),
        )
        .child(
            div()
                .text_size(px(15.))
                .font_weight(FontWeight::SEMIBOLD)
                .whitespace_nowrap()
                .child(period(ui, days)),
        )
        .child(div().flex_1().min_w(px(4.)))
        .child(segmented(
            "cal-mode",
            Mode::ALL.iter().map(|&m| (m, m.label())).collect(),
            ui.mode,
            false,
            theme,
            cx,
            |this, mode, _| set_mode(this, mode),
        ))
        .child(
            widgets::icon_button("cal-refresh", Icon::Refresh, theme)
                .tooltip(widgets::tooltip(
                    tr("ui.calendar_screen.refresh_calendar"),
                    *theme,
                ))
                .on_click(cx.listener(|this, _, _, _| send(this, CalendarAction::Refresh))),
        )
        .child(widgets::disabled(
            primary_icon_button(
                "cal-new",
                Icon::Plus,
                tr("ui.calendar_screen.new_event"),
                theme,
            )
            .when(!creating_blocked, |d| {
                d.on_click(cx.listener(|this, _, _, cx| {
                    let anchor = this.ui.calendar.anchor;
                    new_event(this, anchor);
                    cx.notify();
                }))
            }),
            creating_blocked,
        ))
        .into_any_element()
}

fn error_banner(error: String, busy: bool, theme: &Theme, cx: &mut Context<Oxide>) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap_2()
        .p_3()
        .bg(theme.error_background)
        .text_size(px(12.))
        .text_color(theme.error_text)
        .child(icon(Icon::Alert, px(14.), theme.error_text))
        .child(div().flex_1().child(error))
        .child(widgets::disabled(
            div()
                .id("cal-error-retry")
                .px_2()
                .py_1()
                .rounded(px(6.))
                .font_weight(FontWeight::MEDIUM)
                .cursor_pointer()
                .hover(|s| s.underline())
                .when(!busy, |d| {
                    d.on_click(cx.listener(|this, _, _, _| send(this, CalendarAction::Refresh)))
                })
                .child(tr("ui.calendar_screen.refresh")),
            busy,
        ))
        .into_any_element()
}

/// Today tinted, weekends on the window background.
fn column_tint(day: Day, theme: &Theme) -> Option<Hsla> {
    if day == Day::today() {
        Some(theme.accent_soft.opacity(0.35))
    } else if day.is_weekend() {
        Some(theme.background)
    } else {
        None
    }
}

fn timeline(
    this: &mut Oxide,
    days: &[Day],
    model: &CalendarView,
    theme: &Theme,
    window: &mut Window,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    let mode = this.ui.calendar.mode;
    // The columns fill the width, down to a minimum past which the grid scrolls sideways.
    let side = if model.loading_detail || model.selected.is_some() {
        331.
    } else {
        0.
    };
    let available = f32::from(window.viewport_size().width) - 73. - 233. - side;
    let minimum = if mode == Mode::Day { 400. } else { 130. };
    let width = ((available - 54.) / days.len() as f32).max(minimum);
    let scroll = this.ui.calendar.timeline_scroll.clone();
    if this.ui.calendar.scroll_pending {
        this.ui.calendar.scroll_pending = false;
        scroll.set_offset(point(px(0.), px(-8. * HOUR)));
    }
    let today = Day::today();
    let index = &this.ui.calendar.index;

    let mut header = div().flex().child(div().w(px(54.)).flex_none());
    for (i, &day) in days.iter().enumerate() {
        let is_today = day == today;
        let banners: Vec<AnyElement> = index
            .entries(day)
            .into_iter()
            .filter(|e| e.is_banner(day))
            .enumerate()
            .map(|(j, e)| {
                div()
                    .w_full()
                    .px(px(2.))
                    .child(parts::banner_button(
                        SharedString::from(format!("cal-banner-{i}-{j}")),
                        &e.event,
                        model,
                        theme,
                        cx,
                    ))
                    .into_any_element()
            })
            .collect();
        header = header.child(
            div()
                .w(px(width))
                .flex_none()
                .pb_2()
                .flex()
                .flex_col()
                .gap_1()
                .border_l_1()
                .border_color(theme.border)
                .when_some(column_tint(day, theme), |d, tint| d.bg(tint))
                .child(
                    div()
                        .id(("cal-day-head", i))
                        .py(px(6.))
                        .flex()
                        .flex_col()
                        .items_center()
                        .gap(px(2.))
                        .cursor_pointer()
                        .tooltip(widgets::tooltip(
                            tr("ui.calendar_screen.show_this_day"),
                            *theme,
                        ))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            set_anchor(this, day);
                            set_mode(this, Mode::Day);
                            cx.notify();
                        }))
                        .child(
                            div()
                                .text_size(px(11.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(if is_today {
                                    theme.accent_text
                                } else {
                                    theme.muted
                                })
                                .child(dates::weekday_short(day.weekday())),
                        )
                        .child(
                            div()
                                .size(px(30.))
                                .rounded_full()
                                .flex()
                                .items_center()
                                .justify_center()
                                .text_size(px(16.))
                                .font_weight(if is_today {
                                    FontWeight::BOLD
                                } else {
                                    FontWeight::MEDIUM
                                })
                                .text_color(if is_today { gpui::white() } else { theme.text })
                                .when(is_today, |d| d.bg(theme.accent))
                                .child(day.day().to_string()),
                        ),
                )
                .children(banners),
        );
    }

    let hours = div()
        .w(px(54.))
        .flex_none()
        .flex()
        .flex_col()
        .children((0..24).map(|hour| {
            div()
                .h(px(HOUR))
                .flex()
                .justify_center()
                .text_size(px(10.))
                .line_height(px(12.))
                .text_color(theme.muted)
                .child(dates::hour_label(hour))
        }));
    let mut grid = div().flex().h(px(DAY_HEIGHT)).child(hours);
    let now = dates::now();
    for (i, &day) in days.iter().enumerate() {
        let entries: Vec<_> = index
            .entries(day)
            .into_iter()
            .filter(|e| !e.is_banner(day))
            .collect();
        let spans: Vec<(i64, i64)> = entries.iter().map(|e| (e.start, e.end)).collect();
        let items = layout(&spans, day);
        let slots = div()
            .absolute()
            .inset_0()
            .flex()
            .flex_col()
            .children((0..48u32).map(|slot| {
                let accent = theme.accent;
                div()
                    .id(("cal-slot", i * 48 + slot as usize))
                    .h(px(HOUR / 2.))
                    .flex_none()
                    .when(slot % 2 == 0, |d| d.border_t_1().border_color(theme.border))
                    .cursor_pointer()
                    .hover(move |s| s.bg(accent.opacity(0.08)))
                    .tooltip(widgets::tooltip(tr("ui.calendar_screen.new_event"), *theme))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        create_event(this, day.at(slot / 2, slot % 2 * 30));
                        cx.notify();
                    }))
            }));
        let blocks = items.iter().map(|item| {
            let entry = entries[item.ix];
            let columns = item.columns as f32;
            let w = (width - 4.) / columns - 3.;
            let x = item.column as f32 * (width - 4.) / columns + 2.;
            div()
                .absolute()
                .left(px(x))
                .top(px(item.start))
                .w(px(w))
                .h(px((item.end - item.start).max(24.)))
                .child(parts::event_button(
                    SharedString::from(format!("cal-event-{i}-{}", item.ix)),
                    &entry.event,
                    entry.start,
                    model,
                    true,
                    theme,
                    cx,
                ))
        });
        let now_line = (day == today).then(|| {
            let y = (now - day.start()) as f32 / 60. * HOUR / 60.;
            div()
                .absolute()
                .left(px(-4.))
                .right_0()
                .top(px(y - 4.))
                .h(px(8.))
                .flex()
                .items_center()
                .child(
                    div()
                        .size(px(8.))
                        .flex_none()
                        .rounded_full()
                        .bg(theme.danger),
                )
                .child(div().flex_1().h(px(1.5)).bg(theme.danger))
        });
        grid = grid.child(
            div()
                .relative()
                .w(px(width))
                .flex_none()
                .h(px(DAY_HEIGHT))
                .border_l_1()
                .border_color(theme.border)
                .when_some(column_tint(day, theme), |d, tint| d.bg(tint))
                .child(slots)
                .children(blocks)
                .children(now_line),
        );
    }

    div()
        .id("cal-timeline")
        .size_full()
        .overflow_x_scroll()
        .child(
            div()
                .w(px(54. + width * days.len() as f32))
                .h_full()
                .flex()
                .flex_col()
                .child(header)
                .child(widgets::divider(theme))
                .child(
                    div()
                        .id("cal-timeline-body")
                        .flex_1()
                        .min_h_0()
                        .overflow_y_scroll()
                        .track_scroll(&scroll)
                        .child(grid),
                ),
        )
        .into_any_element()
}

fn month_grid(
    this: &Oxide,
    days: &[Day],
    model: &CalendarView,
    theme: &Theme,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    let anchor = this.ui.calendar.anchor;
    let index = &this.ui.calendar.index;
    let today = Day::today();
    let names = div().flex().children(days[..7].iter().map(|d| {
        div()
            .flex_1()
            .p_2()
            .flex()
            .justify_center()
            .text_size(px(12.))
            .font_weight(FontWeight::SEMIBOLD)
            .child(dates::weekday_wide(d.weekday()))
    }));
    let mut rows = div().min_h_full().flex().flex_col();
    for week in days.chunks(7) {
        let mut row = div().flex_1().min_h(px(95.)).flex();
        for &day in week {
            let is_today = day == today;
            let events = index.entries(day);
            let hover = theme.input;
            row = row.child(
                div()
                    .flex_1()
                    .min_w_0()
                    .p(px(7.))
                    .flex()
                    .flex_col()
                    .gap_1()
                    .border_r_1()
                    .border_b_1()
                    .border_color(theme.border)
                    .bg(if day.same_month(anchor) {
                        theme.surface
                    } else {
                        theme.background
                    })
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .child(
                                div()
                                    .id(("cal-month-day", day.0 as usize))
                                    .text_size(px(13.))
                                    .cursor_pointer()
                                    .when(is_today, |d| {
                                        d.font_weight(FontWeight::BOLD).text_color(theme.accent)
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        set_anchor(this, day);
                                        set_mode(this, Mode::Day);
                                        cx.notify();
                                    }))
                                    .child(day.day().to_string()),
                            )
                            .child(div().flex_1())
                            .child(
                                div()
                                    .id(("cal-month-add", day.0 as usize))
                                    .size(px(16.))
                                    .rounded(px(4.))
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .cursor_pointer()
                                    .hover(move |s| s.bg(hover))
                                    .tooltip(widgets::tooltip(
                                        tr("ui.calendar_screen.new_event_on_this_day"),
                                        *theme,
                                    ))
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        new_event(this, day);
                                        cx.notify();
                                    }))
                                    .child(icon(Icon::Plus, px(10.), theme.text_soft)),
                            ),
                    )
                    .children(events.iter().enumerate().map(|(j, e)| {
                        parts::event_button(
                            SharedString::from(format!("cal-month-event-{}-{j}", day.0)),
                            &e.event,
                            e.start,
                            model,
                            false,
                            theme,
                            cx,
                        )
                    })),
            );
        }
        rows = rows.child(row);
    }
    div()
        .size_full()
        .flex()
        .flex_col()
        .child(names)
        .child(
            div()
                .id("cal-month")
                .flex_1()
                .min_h_0()
                .overflow_y_scroll()
                .child(rows),
        )
        .into_any_element()
}

/// Dialogs and floating panels of this module, drawn over the window.
pub fn overlay(
    this: &mut Oxide,
    window: &mut Window,
    cx: &mut Context<Oxide>,
) -> Option<AnyElement> {
    if !showing(this) {
        hide(this);
        return None;
    }
    calendar_editor::sync(this, window, cx);
    if let Some(editor) = calendar_editor::modal(this, window, cx) {
        return Some(editor);
    }
    let kind = this.ui.calendar.destructive?;
    let theme = this.theme;
    let (question, confirm) = match kind {
        Destructive::Cancel => (
            tr("ui.calendar_screen.cancel_the_meeting_and_notify_attendees"),
            tr("ui.calendar_screen.cancel_meeting"),
        ),
        Destructive::Delete => (
            tr("ui.calendar_screen.delete_this_event_from_your_calendar"),
            tr("ui.calendar_screen.delete"),
        ),
    };
    let content = div()
        .p(px(20.))
        .flex()
        .flex_col()
        .gap_4()
        .child(
            div()
                .text_size(px(14.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(question),
        )
        .child(
            div()
                .flex()
                .justify_end()
                .gap_2()
                .child(
                    widgets::button(
                        "cal-confirm-dismiss",
                        text("Annuler", "Cancel"),
                        ButtonKind::Outline,
                        &theme,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.ui.calendar.destructive = None;
                        cx.notify();
                    })),
                )
                .child(
                    widgets::button("cal-confirm", confirm, ButtonKind::Danger, &theme)
                        .on_click(cx.listener(|this, _, _, cx| confirm_destructive(this, cx))),
                ),
        );
    let entity = cx.entity().downgrade();
    Some(widgets::modal(
        "cal-confirm-dialog",
        px(380.),
        content,
        &theme,
        move |_, cx| {
            let _ = entity.update(cx, |this, cx| {
                this.ui.calendar.destructive = None;
                cx.notify();
            });
        },
    ))
}

fn confirm_destructive(this: &mut Oxide, cx: &mut Context<Oxide>) {
    let series = this.ui.calendar.whole_series;
    match this.ui.calendar.destructive.take() {
        Some(Destructive::Cancel) => send(this, CalendarAction::Cancel { series }),
        Some(Destructive::Delete) => send(this, CalendarAction::Delete { series }),
        None => {}
    }
    cx.notify();
}

/// A step of `OXIDE_SCRIPT` this module understands (see `screens::run_script`); false if not its own.
/// `calendar:<agenda|day|week|month|prev|next|today>`, `calendar:goto 2026-10-25`, `calendar:open <n-th event>`,
/// `calendar:new`, `calendar:slot <hour>`, `calendar:edit`, `calendar:edit-series`, `calendar:series`,
/// `calendar:respond <Accept|TentativelyAccept|Decline>`, `calendar:cancel`, `calendar:delete`, `calendar:confirm`,
/// `calendar:close`, and the editor's `calendar:ed <picker|repeat|unit|ends|all-day|title|query|attendee|save|close> [arg]`.
pub fn debug_step(
    this: &mut Oxide,
    step: &str,
    _window: &mut Window,
    cx: &mut Context<Oxide>,
) -> bool {
    let Some(rest) = step.strip_prefix("calendar:") else {
        return false;
    };
    let (verb, arg) = rest.split_once(' ').unwrap_or((rest, ""));
    match verb {
        "agenda" => set_mode(this, Mode::Agenda),
        "day" => set_mode(this, Mode::Day),
        "week" => set_mode(this, Mode::Week),
        "month" => set_mode(this, Mode::Month),
        "prev" => shift(this, -1),
        "next" => shift(this, 1),
        "today" => set_anchor(this, Day::today()),
        "goto" => match Day::parse_iso(arg) {
            Some(day) => set_anchor(this, day),
            None => return false,
        },
        "open" => {
            let n: usize = arg.parse().unwrap_or(0);
            let Some(id) = this
                .ui
                .calendar
                .index
                .all
                .get(n)
                .map(|e| e.event.id.clone())
            else {
                return false;
            };
            select(this, &id);
        }
        "new" => {
            let anchor = this.ui.calendar.anchor;
            new_event(this, anchor);
        }
        "slot" => {
            let hour: u32 = arg.parse().unwrap_or(9);
            let anchor = this.ui.calendar.anchor;
            create_event(this, anchor.at(hour.min(23), 0));
        }
        "edit" => send(this, CalendarAction::Edit { series: false }),
        "edit-series" => send(this, CalendarAction::Edit { series: true }),
        "series" => this.ui.calendar.whole_series = !this.ui.calendar.whole_series,
        "respond" => {
            let series = this.ui.calendar.whole_series;
            send(
                this,
                CalendarAction::Respond {
                    response: arg.to_owned(),
                    series,
                },
            );
        }
        "cancel" => this.ui.calendar.destructive = Some(Destructive::Cancel),
        "delete" => this.ui.calendar.destructive = Some(Destructive::Delete),
        "confirm" => confirm_destructive(this, cx),
        "close" => send(this, CalendarAction::CloseDetail),
        "state" => eprintln!(
            "calendar: nav {:?}, events {}, selected {:?}, draft {}",
            this.shell.as_ref().map(|s| s.nav),
            this.calendar.as_ref().map_or(0, |c| c.events.len()),
            this.calendar
                .as_ref()
                .and_then(|c| c.selected.as_ref().map(|e| e.id.clone())),
            this.calendar.as_ref().is_some_and(|c| c.draft.is_some()),
        ),
        "ed" => {
            let (verb, arg) = arg.split_once(' ').unwrap_or((arg, ""));
            return calendar_editor::debug_step(this, verb, arg, cx);
        }
        _ => return false,
    }
    cx.notify();
    true
}

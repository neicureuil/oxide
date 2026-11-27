//! The event form (`CalendarEditor.swift`): title, dates, place, Teams meeting, repeat, description and attendees.
//! The form is local while it is open; typing sends it to the core once it pauses, saving sends it at once.
//! Its pickers open inline under their row: a floating panel cannot be drawn from inside the modal.
use crate::calendar::send;
use crate::calendar_dates::{self as dates, Day, iso, parse_iso};
use crate::calendar_parts::{MonthGrid, Pick, month_grid};
use crate::icons::{Icon, icon};
use crate::input::{InputEvent, TextInput};
use crate::oxide::{Oxide, tr, trf};
use crate::theme::Theme;
use crate::widgets::{self, ButtonKind};
use gpui::{
    AnyElement, Context, Div, ElementId, Entity, FontWeight, ScrollHandle, Stateful, Window, div,
    prelude::*, px,
};
use std::rc::Rc;
use std::time::Duration;
use teams_core::{CalendarAction, CalendarDraft, CalendarPerson, CalendarView, Repeat, RepeatUnit};

/// Which inline picker is open.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Picker {
    StartDate,
    StartTime,
    EndDate,
    EndTime,
    Repeat,
    Unit,
    SeriesEnd,
}

pub struct Editor {
    pub subject: Entity<TextInput>,
    pub location: Entity<TextInput>,
    pub description: Entity<TextInput>,
    pub query: Entity<TextInput>,
    /// The form, taken from the core's draft when the editor opened; `None` while closed.
    pub form: Option<CalendarDraft>,
    /// Whether the next attendee added is optional.
    pub optional: bool,
    pub picker: Option<Picker>,
    /// The month a date picker shows.
    pub picker_month: Day,
    pub time_scroll: ScrollHandle,
    /// Bumped to drop a pending draft send or people search.
    draft_generation: u64,
    query_generation: u64,
}

impl Editor {
    pub fn new(_window: &mut Window, cx: &mut Context<Oxide>) -> Self {
        let subject = cx.new(|cx| TextInput::new(cx, tr("ui.calendar_screen.title")));
        let location = cx.new(|cx| TextInput::new(cx, tr("ui.calendar_screen.location")));
        let description = cx.new(|cx| TextInput::new(cx, "").multiline(false));
        let query = cx.new(|cx| TextInput::new(cx, tr("ui.calendar_screen.name_or_email_address")));
        cx.subscribe(
            &subject,
            |this: &mut Oxide, input, event: &InputEvent, cx| {
                let value = input.read(cx).text().to_owned();
                field_event(this, event, cx, move |form| form.subject = value);
            },
        )
        .detach();
        cx.subscribe(
            &location,
            |this: &mut Oxide, input, event: &InputEvent, cx| {
                let value = input.read(cx).text().to_owned();
                field_event(this, event, cx, move |form| form.location = value);
            },
        )
        .detach();
        cx.subscribe(
            &description,
            |this: &mut Oxide, input, event: &InputEvent, cx| {
                let value = input.read(cx).text().to_owned();
                field_event(this, event, cx, move |form| form.description = value);
            },
        )
        .detach();
        cx.subscribe(&query, |this: &mut Oxide, input, event: &InputEvent, cx| {
            let typed = input.read(cx).text().to_owned();
            match event {
                InputEvent::Changed => query_changed(this, typed, cx),
                InputEvent::Submit => {
                    if typed.contains('@') {
                        add(
                            this,
                            CalendarPerson {
                                address: typed,
                                ..Default::default()
                            },
                            cx,
                        );
                    } else {
                        send(this, CalendarAction::SearchPeople { query: typed });
                    }
                }
                InputEvent::Cancel => close(this, cx),
                _ => {}
            }
        })
        .detach();
        Self {
            subject,
            location,
            description,
            query,
            form: None,
            optional: false,
            picker: None,
            picker_month: Day::today(),
            time_scroll: ScrollHandle::new(),
            draft_generation: 0,
            query_generation: 0,
        }
    }
}

fn field_event(
    this: &mut Oxide,
    event: &InputEvent,
    cx: &mut Context<Oxide>,
    apply: impl FnOnce(&mut CalendarDraft),
) {
    match event {
        InputEvent::Changed => edit(this, cx, apply),
        InputEvent::Submit => save(this, cx),
        InputEvent::Cancel => close(this, cx),
        _ => {}
    }
}

fn model(this: &Oxide) -> Option<&CalendarView> {
    this.calendar.as_ref()
}

/// Opens the form when the core starts a draft, forgets it when the core closes it.
pub fn sync(this: &mut Oxide, window: &mut Window, cx: &mut Context<Oxide>) {
    let draft = this.calendar.as_ref().and_then(|c| c.draft.clone());
    let ed = &mut this.ui.calendar.editor;
    match (draft, ed.form.is_some()) {
        (Some(draft), false) => {
            let texts = [
                (ed.subject.clone(), draft.subject.clone()),
                (ed.location.clone(), draft.location.clone()),
                (ed.description.clone(), draft.description.clone()),
                (ed.query.clone(), String::new()),
            ];
            ed.picker_month = parse_iso(&draft.start)
                .map(Day::of)
                .unwrap_or_else(Day::today);
            ed.form = Some(draft);
            ed.optional = false;
            ed.picker = None;
            for (input, value) in texts {
                input.update(cx, |i, cx| i.set_text(value, None, cx));
            }
            ed.subject.read(cx).focus(window);
        }
        (None, true) => {
            ed.form = None;
            ed.picker = None;
            ed.draft_generation += 1;
            ed.query_generation += 1;
        }
        _ => {}
    }
}

/// Changes the form and sends it to the core once changes pause.
fn edit(this: &mut Oxide, cx: &mut Context<Oxide>, apply: impl FnOnce(&mut CalendarDraft)) {
    if model(this).is_some_and(|m| m.busy) || this.ui.calendar.editor.form.is_none() {
        return;
    }
    let ed = &mut this.ui.calendar.editor;
    let form = ed.form.as_mut().unwrap();
    let before = form.clone();
    apply(form);
    if *form == before {
        return;
    }
    ed.draft_generation += 1;
    let generation = ed.draft_generation;
    cx.spawn(async move |this, cx| {
        cx.background_executor()
            .timer(Duration::from_millis(300))
            .await;
        let _ = this.update(cx, |this, _| {
            let ed = &this.ui.calendar.editor;
            if ed.draft_generation == generation
                && let Some(value) = ed.form.clone()
            {
                send(this, CalendarAction::Draft { value });
            }
        });
    })
    .detach();
    cx.notify();
}

fn can_save(this: &Oxide) -> bool {
    let busy = model(this).is_some_and(|m| m.busy || m.uncertain);
    let ed = &this.ui.calendar.editor;
    !busy
        && ed
            .form
            .as_ref()
            .is_some_and(|f| !f.subject.trim().is_empty())
}

pub fn save(this: &mut Oxide, cx: &mut Context<Oxide>) {
    if !can_save(this) {
        return;
    }
    let ed = &mut this.ui.calendar.editor;
    ed.draft_generation += 1;
    let value = ed.form.clone().unwrap_or_default();
    send(this, CalendarAction::Draft { value });
    send(this, CalendarAction::Save);
    cx.notify();
}

pub fn close(this: &mut Oxide, cx: &mut Context<Oxide>) {
    if model(this).is_some_and(|m| m.busy) {
        return;
    }
    this.ui.calendar.editor.draft_generation += 1;
    send(this, CalendarAction::CloseEditor);
    cx.notify();
}

/// Searches as the name is typed, once typing pauses; an address is added, not searched.
fn query_changed(this: &mut Oxide, query: String, cx: &mut Context<Oxide>) {
    let ed = &mut this.ui.calendar.editor;
    ed.query_generation += 1;
    let typed = query.trim().to_owned();
    if typed.chars().count() < 2 || typed.contains('@') {
        send(
            this,
            CalendarAction::SearchPeople {
                query: String::new(),
            },
        );
        cx.notify();
        return;
    }
    let generation = ed.query_generation;
    cx.spawn(async move |this, cx| {
        cx.background_executor()
            .timer(Duration::from_millis(250))
            .await;
        let _ = this.update(cx, |this, _| {
            if this.ui.calendar.editor.query_generation == generation {
                send(this, CalendarAction::SearchPeople { query: typed });
            }
        });
    })
    .detach();
    cx.notify();
}

fn add(this: &mut Oxide, person: CalendarPerson, cx: &mut Context<Oxide>) {
    let address = person.address.trim().to_owned();
    let Some(form) = this.ui.calendar.editor.form.as_ref() else {
        return;
    };
    if !address.contains('@')
        || form
            .attendees
            .iter()
            .any(|a| a.address.eq_ignore_ascii_case(&address))
    {
        return;
    }
    let optional = this.ui.calendar.editor.optional;
    edit(this, cx, |form| {
        form.attendees.push(CalendarPerson {
            address,
            name: person.name,
            optional,
            response: String::new(),
        })
    });
    let ed = &mut this.ui.calendar.editor;
    ed.query_generation += 1;
    ed.query.update(cx, |i, cx| i.set_text("", None, cx));
    send(
        this,
        CalendarAction::SearchPeople {
            query: String::new(),
        },
    );
}

/// The date a field shows: an all-day event's end is the day after its last day.
fn field_time(form: &CalendarDraft, start: bool) -> i64 {
    let t = parse_iso(if start { &form.start } else { &form.end }).unwrap_or_else(dates::now);
    if form.all_day && !start {
        let (day, h, m) = dates::local(t);
        day.add(-1).at(h, m)
    } else {
        t
    }
}

/// Sets a field, keeping the end after the start.
fn set_field_time(form: &mut CalendarDraft, start: bool, value: i64) {
    let d = if form.all_day {
        Day::of(value).start()
    } else {
        value
    };
    if start {
        form.start = iso(d);
        if parse_iso(&form.end).is_some_and(|end| end <= d) {
            form.end = iso(if form.all_day {
                Day::of(d).add(1).start()
            } else {
                d + 30 * 60
            });
        }
    } else {
        form.end = iso(if form.all_day {
            Day::of(d).add(1).start()
        } else {
            d
        });
    }
}

fn set_all_day(form: &mut CalendarDraft, all_day: bool) {
    form.all_day = all_day;
    if all_day {
        let start = Day::of(parse_iso(&form.start).unwrap_or_else(dates::now));
        form.start = iso(start.start());
        form.end = iso(start.add(1).start());
    }
}

fn set_repeat(form: &mut CalendarDraft, repeat: Repeat) {
    let r = &mut form.recurrence;
    r.repeat = repeat;
    if repeat == Repeat::Custom && r.interval == 0 {
        let start = Day::of(parse_iso(&form.start).unwrap_or_else(dates::now));
        r.interval = 1;
        r.unit = RepeatUnit::Week;
        r.weekdays = vec![start.weekday()];
    }
}

fn repeat_label(repeat: Repeat, start: Day) -> String {
    match repeat {
        Repeat::Never => tr("ui.calendar_screen.does_not_repeat"),
        Repeat::Weekdays => tr("ui.calendar_screen.every_weekday"),
        Repeat::Daily => tr("ui.calendar_screen.daily"),
        Repeat::Weekly => trf(
            "ui.calendar_screen.weekly_on",
            &[("day", &dates::weekday_wide(start.weekday()))],
        ),
        Repeat::Monthly => tr("ui.calendar_screen.monthly"),
        Repeat::Yearly => trf(
            "ui.calendar_screen.yearly_on",
            &[("date", &dates::day_month_wide(start))],
        ),
        Repeat::Custom => tr("ui.calendar_screen.custom"),
    }
}

fn unit_label(unit: RepeatUnit) -> String {
    tr(match unit {
        RepeatUnit::Day => "ui.calendar_screen.days",
        RepeatUnit::Week => "ui.calendar_screen.weeks",
        RepeatUnit::Month => "ui.calendar_screen.months",
        RepeatUnit::Year => "ui.calendar_screen.years",
    })
}

fn toggle_picker(this: &mut Oxide, picker: Picker, cx: &mut Context<Oxide>) {
    let ed = &mut this.ui.calendar.editor;
    ed.picker = if ed.picker == Some(picker) {
        None
    } else {
        Some(picker)
    };
    if let (Some(p), Some(form)) = (ed.picker, ed.form.as_ref()) {
        let shown = match p {
            Picker::StartDate | Picker::StartTime => Some(field_time(form, true)),
            Picker::EndDate | Picker::EndTime => Some(field_time(form, false)),
            Picker::SeriesEnd => Some(
                Day::parse_iso(&form.recurrence.end_date)
                    .map(Day::start)
                    .unwrap_or_else(|| field_time(form, true)),
            ),
            _ => None,
        };
        if let Some(t) = shown {
            let (day, h, m) = dates::local(t);
            ed.picker_month = day;
            if matches!(p, Picker::StartTime | Picker::EndTime) {
                // Rows are 26 points: the current time shows as the list's third row.
                let row = ((h * 60 + m) / 15) as f32;
                ed.time_scroll
                    .set_offset(gpui::point(px(0.), px(-(row - 2.).max(0.) * 26.)));
            }
        }
    }
    cx.notify();
}

pub fn set_picker(this: &mut Oxide, picker: Option<Picker>, cx: &mut Context<Oxide>) {
    this.ui.calendar.editor.picker = None;
    if let Some(p) = picker {
        toggle_picker(this, p, cx);
    }
}

/// A field that opens a picker: its value and a chevron.
fn dropdown(id: impl Into<ElementId>, label: String, open: bool, theme: &Theme) -> Stateful<Div> {
    let hover = theme.input;
    div()
        .id(id)
        .h(px(28.))
        .px(px(10.))
        .flex()
        .flex_none()
        .items_center()
        .gap_2()
        .rounded(px(7.))
        .border_1()
        .border_color(if open {
            theme.accent
        } else {
            theme.border_strong
        })
        .bg(theme.surface)
        .text_size(px(13.))
        .cursor_pointer()
        .hover(move |s| s.bg(hover))
        .child(label)
        .child(icon(Icon::ChevronDown, px(11.), theme.muted))
}

/// A row of the grouped form: its label on the left, the control on the right.
fn row(label: String, control: impl IntoElement, theme: &Theme) -> Div {
    div()
        .min_h(px(40.))
        .px_3()
        .py(px(6.))
        .flex()
        .items_center()
        .gap_3()
        .child(
            div()
                .flex_none()
                .text_size(px(13.))
                .text_color(theme.text)
                .child(label),
        )
        .child(
            div()
                .flex_1()
                .flex()
                .justify_end()
                .items_center()
                .gap_2()
                .child(control),
        )
}

/// A group of rows with hairlines between them (`Form` in the grouped style).
fn group(rows: Vec<AnyElement>, theme: &Theme) -> Div {
    let mut g = div()
        .flex()
        .flex_col()
        .rounded(px(10.))
        .border_1()
        .border_color(theme.border)
        .bg(theme.surface);
    for (i, r) in rows.into_iter().enumerate() {
        if i > 0 {
            g = g.child(div().mx_3().child(widgets::divider(theme)));
        }
        g = g.child(r);
    }
    g
}

/// A text field without a frame, aligned right in its row, as macOS grouped forms draw them.
fn inline_field(input: &Entity<TextInput>, theme: &Theme, cx: &Context<Oxide>) -> Div {
    div()
        .flex_1()
        .max_w(px(420.))
        .child(widgets::text_field(input, theme, cx))
}

fn date_and_time(
    form: &CalendarDraft,
    start: bool,
    picker: Option<Picker>,
    theme: &Theme,
    cx: &mut Context<Oxide>,
) -> Div {
    let t = field_time(form, start);
    let (day, h, m) = dates::local(t);
    let (date_picker, time_picker) = if start {
        (Picker::StartDate, Picker::StartTime)
    } else {
        (Picker::EndDate, Picker::EndTime)
    };
    div()
        .flex()
        .gap_2()
        .child(
            dropdown(
                if start {
                    "cal-ed-start-date"
                } else {
                    "cal-ed-end-date"
                },
                dates::short_date(day),
                picker == Some(date_picker),
                theme,
            )
            .on_click(cx.listener(move |this, _, _, cx| toggle_picker(this, date_picker, cx))),
        )
        .when(!form.all_day, |d| {
            d.child(
                dropdown(
                    if start {
                        "cal-ed-start-time"
                    } else {
                        "cal-ed-end-time"
                    },
                    dates::time_of(h, m),
                    picker == Some(time_picker),
                    theme,
                )
                .on_click(cx.listener(move |this, _, _, cx| toggle_picker(this, time_picker, cx))),
            )
        })
}

/// The month grid of a date picker, with its own month arrows.
fn date_picker(
    this: &Oxide,
    picker: Picker,
    form: &CalendarDraft,
    theme: &Theme,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    let month = this.ui.calendar.editor.picker_month;
    let start_day = Day::of(field_time(form, true));
    let selected = match picker {
        Picker::StartDate => start_day,
        Picker::EndDate => Day::of(field_time(form, false)),
        _ => Day::parse_iso(&form.recurrence.end_date).unwrap_or(start_day),
    };
    let pick: Pick = Rc::new(move |this, day, _, cx| {
        match picker {
            Picker::SeriesEnd => edit(this, cx, |form| form.recurrence.end_date = day.iso()),
            _ => {
                let start = picker == Picker::StartDate;
                edit(this, cx, |form| {
                    let (_, h, m) = dates::local(field_time(form, start));
                    set_field_time(form, start, day.at(h, m));
                });
            }
        }
        this.ui.calendar.editor.picker = None;
        cx.notify();
    });
    let series_end = picker == Picker::SeriesEnd;
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
            widgets::icon_button_sized(
                "cal-ed-month-prev",
                Icon::ChevronLeft,
                22.,
                11.,
                false,
                theme,
            )
            .on_click(cx.listener(|this, _, _, cx| {
                let ed = &mut this.ui.calendar.editor;
                ed.picker_month = ed.picker_month.add_months(-1);
                cx.notify();
            })),
        )
        .child(
            widgets::icon_button_sized(
                "cal-ed-month-next",
                Icon::ChevronRight,
                22.,
                11.,
                false,
                theme,
            )
            .on_click(cx.listener(|this, _, _, cx| {
                let ed = &mut this.ui.calendar.editor;
                ed.picker_month = ed.picker_month.add_months(1);
                cx.notify();
            })),
        );
    let grid = month_grid(
        MonthGrid {
            id: "cal-ed-day",
            month,
            shown: &|d| d == selected,
            busy: &|_| false,
            enabled: &|d| !series_end || d >= start_day,
            pick,
        },
        theme,
        cx,
    );
    picker_panel(
        div()
            .w(px(240.))
            .flex()
            .flex_col()
            .gap(px(6.))
            .child(header)
            .child(grid),
        theme,
    )
}

/// Quarter hours of the day; the current one is highlighted.
fn time_picker(
    this: &Oxide,
    start: bool,
    form: &CalendarDraft,
    theme: &Theme,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    let (_, ch, cm) = dates::local(field_time(form, start));
    let hover = theme.input;
    let list = div()
        .id("cal-ed-times")
        .w(px(140.))
        .h(px(180.))
        .overflow_y_scroll()
        .track_scroll(&this.ui.calendar.editor.time_scroll)
        .children((0..96u32).map(|q| {
            let (h, m) = (q / 4, q % 4 * 15);
            let current = h == ch && m == cm;
            div()
                .id(("cal-ed-time", q as usize))
                .h(px(26.))
                .px_2()
                .flex()
                .items_center()
                .rounded(px(5.))
                .text_size(px(13.))
                .cursor_pointer()
                .when(current, |d| {
                    d.bg(theme.accent_soft)
                        .text_color(theme.accent_text)
                        .font_weight(FontWeight::SEMIBOLD)
                })
                .when(!current, move |d| d.hover(move |s| s.bg(hover)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    edit(this, cx, |form| {
                        let (day, _, _) = dates::local(field_time(form, start));
                        set_field_time(form, start, day.at(h, m));
                    });
                    this.ui.calendar.editor.picker = None;
                    cx.notify();
                }))
                .child(dates::time_of(h, m))
        }));
    picker_panel(list, theme)
}

fn picker_panel(content: impl IntoElement, theme: &Theme) -> AnyElement {
    div()
        .px_3()
        .pb_3()
        .flex()
        .justify_end()
        .child(
            div()
                .p_2()
                .rounded(px(10.))
                .border_1()
                .border_color(theme.border)
                .bg(theme.background)
                .child(content),
        )
        .into_any_element()
}

/// A list of choices under a dropdown.
fn choices<T: Copy + PartialEq + 'static>(
    id: &'static str,
    options: Vec<(T, String)>,
    current: T,
    theme: &Theme,
    cx: &mut Context<Oxide>,
    apply: fn(&mut CalendarDraft, T),
) -> AnyElement {
    let hover = theme.input;
    picker_panel(
        div()
            .min_w(px(220.))
            .flex()
            .flex_col()
            .children(options.into_iter().enumerate().map(|(i, (value, label))| {
                let active = value == current;
                div()
                    .id((id, i))
                    .h(px(28.))
                    .px_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .rounded(px(6.))
                    .text_size(px(13.))
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover))
                    .child(div().w(px(14.)).when(active, |d| {
                        d.child(icon(Icon::Check, px(13.), theme.accent_text))
                    }))
                    .child(label)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        edit(this, cx, |form| apply(form, value));
                        this.ui.calendar.editor.picker = None;
                        cx.notify();
                    }))
            })),
        theme,
    )
}

/// "Repeat": the web client's presets, or a custom rule, and when the series ends.
fn repeat_rows(
    this: &Oxide,
    form: &CalendarDraft,
    theme: &Theme,
    cx: &mut Context<Oxide>,
) -> Vec<AnyElement> {
    let picker = this.ui.calendar.editor.picker;
    let r = &form.recurrence;
    let start = Day::of(parse_iso(&form.start).unwrap_or_else(dates::now));
    let mut rows = vec![
        row(
            tr("ui.calendar_screen.repeat"),
            dropdown(
                "cal-ed-repeat",
                repeat_label(r.repeat, start),
                picker == Some(Picker::Repeat),
                theme,
            )
            .on_click(cx.listener(|this, _, _, cx| toggle_picker(this, Picker::Repeat, cx))),
            theme,
        )
        .into_any_element(),
    ];
    if picker == Some(Picker::Repeat) {
        let options = [
            Repeat::Never,
            Repeat::Weekdays,
            Repeat::Daily,
            Repeat::Weekly,
            Repeat::Monthly,
            Repeat::Yearly,
            Repeat::Custom,
        ]
        .map(|v| (v, repeat_label(v, start)))
        .to_vec();
        rows.push(choices(
            "cal-ed-repeat-choice",
            options,
            r.repeat,
            theme,
            cx,
            set_repeat,
        ));
    }
    if r.repeat == Repeat::Custom {
        let interval = r.interval.max(1);
        let stepper = div()
            .flex()
            .items_center()
            .gap_2()
            .child(div().text_size(px(13.)).child(trf(
                "ui.calendar_screen.every_n",
                &[("count", &interval.to_string())],
            )))
            .child(
                div()
                    .flex()
                    .rounded(px(7.))
                    .border_1()
                    .border_color(theme.border_strong)
                    .child(
                        widgets::disabled(
                            widgets::icon_button_sized(
                                "cal-ed-interval-down",
                                Icon::ChevronDown,
                                24.,
                                11.,
                                false,
                                theme,
                            ),
                            interval <= 1,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            edit(this, cx, |f| {
                                f.recurrence.interval =
                                    f.recurrence.interval.max(1).saturating_sub(1).max(1)
                            })
                        })),
                    )
                    .child(
                        widgets::disabled(
                            widgets::icon_button_sized(
                                "cal-ed-interval-up",
                                Icon::ChevronUp,
                                24.,
                                11.,
                                false,
                                theme,
                            ),
                            interval >= 99,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            edit(this, cx, |f| {
                                f.recurrence.interval = (f.recurrence.interval.max(1) + 1).min(99)
                            })
                        })),
                    ),
            )
            .child(
                dropdown(
                    "cal-ed-unit",
                    unit_label(r.unit),
                    picker == Some(Picker::Unit),
                    theme,
                )
                .min_w(px(120.))
                .on_click(cx.listener(|this, _, _, cx| toggle_picker(this, Picker::Unit, cx))),
            );
        rows.push(
            div()
                .min_h(px(40.))
                .px_3()
                .py(px(6.))
                .flex()
                .items_center()
                .justify_end()
                .child(stepper)
                .into_any_element(),
        );
        if picker == Some(Picker::Unit) {
            let options = [
                RepeatUnit::Day,
                RepeatUnit::Week,
                RepeatUnit::Month,
                RepeatUnit::Year,
            ]
            .map(|u| (u, unit_label(u)))
            .to_vec();
            rows.push(choices(
                "cal-ed-unit-choice",
                options,
                r.unit,
                theme,
                cx,
                |f, u| f.recurrence.unit = u,
            ));
        }
        if r.unit == RepeatUnit::Week {
            let days = div().flex().gap_1().children((0..7u32).map(|day| {
                let on = r.weekdays.contains(&day);
                let hover = theme.input;
                div()
                    .id(("cal-ed-weekday", day as usize))
                    .size(px(28.))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_size(px(12.))
                    .cursor_pointer()
                    .text_color(if on { theme.accent_text } else { theme.muted })
                    .when(on, |d| {
                        d.bg(theme.accent_soft).font_weight(FontWeight::SEMIBOLD)
                    })
                    .when(!on, move |d| d.hover(move |s| s.bg(hover)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        edit(this, cx, |f| {
                            let w = &mut f.recurrence.weekdays;
                            if w.contains(&day) {
                                w.retain(|&x| x != day);
                            } else {
                                w.push(day);
                            }
                        })
                    }))
                    .child(dates::weekday_narrow(day))
            }));
            rows.push(
                div()
                    .min_h(px(40.))
                    .px_3()
                    .py(px(6.))
                    .flex()
                    .items_center()
                    .justify_end()
                    .child(days)
                    .into_any_element(),
            );
        }
        if matches!(r.unit, RepeatUnit::Month | RepeatUnit::Year) {
            let by_weekday = r.by_weekday;
            let radios = div()
                .flex()
                .flex_col()
                .gap_2()
                .text_size(px(13.))
                .child(
                    widgets::radio(
                        "cal-ed-on-day",
                        !by_weekday,
                        trf(
                            "ui.calendar_screen.on_day",
                            &[("day", &start.day().to_string())],
                        ),
                        theme,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        edit(this, cx, |f| f.recurrence.by_weekday = false)
                    })),
                )
                .child(
                    widgets::radio(
                        "cal-ed-on-weekday",
                        by_weekday,
                        tr("ui.calendar_screen.on_the_same_weekday"),
                        theme,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        edit(this, cx, |f| f.recurrence.by_weekday = true)
                    })),
                );
            rows.push(
                div()
                    .px_3()
                    .py_2()
                    .flex()
                    .justify_end()
                    .child(radios)
                    .into_any_element(),
            );
        }
    }
    if r.repeat != Repeat::Never {
        let ends = !r.end_date.is_empty();
        rows.push(
            row(
                tr("ui.calendar_screen.ends_on_a_date"),
                widgets::toggle("cal-ed-ends", ends, theme).on_click(cx.listener(
                    move |this, _, _, cx| {
                        edit(this, cx, |f| {
                            f.recurrence.end_date = if ends {
                                String::new()
                            } else {
                                Day::of(parse_iso(&f.start).unwrap_or_else(dates::now))
                                    .add_months(3)
                                    .iso()
                            };
                        })
                    },
                )),
                theme,
            )
            .into_any_element(),
        );
        if let Some(end) = Day::parse_iso(&r.end_date) {
            rows.push(
                row(
                    tr("ui.calendar_screen.end_of_series"),
                    dropdown(
                        "cal-ed-series-end",
                        dates::short_date(end),
                        picker == Some(Picker::SeriesEnd),
                        theme,
                    )
                    .on_click(
                        cx.listener(|this, _, _, cx| toggle_picker(this, Picker::SeriesEnd, cx)),
                    ),
                    theme,
                )
                .into_any_element(),
            );
            if picker == Some(Picker::SeriesEnd) {
                rows.push(date_picker(this, Picker::SeriesEnd, form, theme, cx));
            }
        }
    }
    rows
}

/// The editor as a dialog over the window.
pub fn modal(this: &mut Oxide, window: &mut Window, cx: &mut Context<Oxide>) -> Option<AnyElement> {
    let theme = this.theme;
    let model = this.calendar.clone().unwrap_or_default();
    let ed = &this.ui.calendar.editor;
    let form = ed.form.clone()?;
    let picker = ed.picker;
    let editing = model.editing;
    let busy = model.busy;

    let mut first = vec![
        row(
            tr("ui.calendar_screen.title"),
            inline_field(&ed.subject, &theme, cx),
            &theme,
        )
        .into_any_element(),
        row(
            tr("ui.calendar_screen.all_day_2"),
            widgets::toggle("cal-ed-all-day", form.all_day, &theme).on_click(cx.listener(
                |this, _, _, cx| {
                    edit(this, cx, |f| {
                        let all_day = !f.all_day;
                        set_all_day(f, all_day)
                    })
                },
            )),
            &theme,
        )
        .into_any_element(),
        row(
            tr("ui.calendar_screen.start"),
            date_and_time(&form, true, picker, &theme, cx),
            &theme,
        )
        .into_any_element(),
    ];
    match picker {
        Some(p @ Picker::StartDate) => first.push(date_picker(this, p, &form, &theme, cx)),
        Some(Picker::StartTime) => first.push(time_picker(this, true, &form, &theme, cx)),
        _ => {}
    }
    first.push(
        row(
            tr("ui.calendar_screen.end"),
            date_and_time(&form, false, picker, &theme, cx),
            &theme,
        )
        .into_any_element(),
    );
    match picker {
        Some(p @ Picker::EndDate) => first.push(date_picker(this, p, &form, &theme, cx)),
        Some(Picker::EndTime) => first.push(time_picker(this, false, &form, &theme, cx)),
        _ => {}
    }
    first.push(
        div()
            .px_3()
            .py_2()
            .child(widgets::muted(
                trf(
                    "ui.calendar_screen.time_zone",
                    &[("identifier", &dates::time_zone_id())],
                ),
                11.,
                &theme,
            ))
            .into_any_element(),
    );
    let ed = &this.ui.calendar.editor;
    first.push(
        row(
            tr("ui.calendar_screen.location"),
            inline_field(&ed.location, &theme, cx),
            &theme,
        )
        .into_any_element(),
    );
    first.push(
        row(
            tr("ui.calendar_screen.teams_meeting"),
            widgets::disabled(
                widgets::toggle("cal-ed-online", form.online, &theme),
                editing,
            )
            .when(!editing, |d| {
                d.on_click(cx.listener(|this, _, _, cx| edit(this, cx, |f| f.online = !f.online)))
            }),
            &theme,
        )
        .into_any_element(),
    );
    if !model.editing_occurrence {
        first.extend(repeat_rows(this, &form, &theme, cx));
    }
    let ed = &this.ui.calendar.editor;
    first.push(
        div()
            .px_3()
            .py_2()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .text_size(px(12.))
                    .child(tr("ui.calendar_screen.description")),
            )
            .child(
                div()
                    .min_h(px(90.))
                    .px_2()
                    .py(px(6.))
                    .rounded(px(4.))
                    .border_1()
                    .border_color(theme.border)
                    .text_size(px(13.))
                    .line_height(px(18.))
                    .child(crate::input::field(
                        &ed.description,
                        widgets::input_colors(&theme),
                        cx,
                    )),
            )
            .into_any_element(),
    );

    let query_text = ed.query.read(cx).text().to_owned();
    let mut people = vec![
        div()
            .px_3()
            .py_2()
            .flex()
            .items_center()
            .gap_2()
            .child(
                div()
                    .flex_1()
                    .child(widgets::text_field(&ed.query, &theme, cx)),
            )
            .child(widgets::disabled(
                widgets::button(
                    "cal-ed-add",
                    tr("ui.calendar_screen.add"),
                    ButtonKind::Outline,
                    &theme,
                )
                .h(px(28.))
                .when(query_text.contains('@'), |d| {
                    d.on_click(cx.listener(|this, _, _, cx| {
                        let address = this
                            .ui
                            .calendar
                            .editor
                            .query
                            .read(cx)
                            .text()
                            .trim()
                            .to_owned();
                        add(
                            this,
                            CalendarPerson {
                                address,
                                ..Default::default()
                            },
                            cx,
                        );
                    }))
                }),
                !query_text.contains('@'),
            ))
            .into_any_element(),
        row(
            tr("ui.calendar_screen.optional_attendee"),
            widgets::toggle("cal-ed-optional", ed.optional, &theme).on_click(cx.listener(
                |this, _, _, cx| {
                    let ed = &mut this.ui.calendar.editor;
                    ed.optional = !ed.optional;
                    cx.notify();
                },
            )),
            &theme,
        )
        .into_any_element(),
    ];
    if model.searching {
        people.push(
            div()
                .px_3()
                .py_2()
                .child(widgets::spinner("cal-ed-searching", 14., theme.muted))
                .into_any_element(),
        );
    }
    if !model.people.is_empty() {
        people.push(
            div()
                .px_3()
                .py_2()
                .flex()
                .flex_col()
                .items_start()
                .gap_1()
                .children(model.people.iter().enumerate().map(|(i, person)| {
                    let found = person.clone();
                    div()
                        .id(("cal-ed-person", i))
                        .text_size(px(13.))
                        .text_color(theme.accent_text)
                        .cursor_pointer()
                        .hover(|s| s.underline())
                        .on_click(cx.listener(move |this, _, _, cx| add(this, found.clone(), cx)))
                        .child(format!("{} · {}", person.name, person.address))
                }))
                .into_any_element(),
        );
    }
    for (i, person) in form.attendees.iter().enumerate() {
        let shown = if person.name.is_empty() {
            person.address.clone()
        } else {
            format!("{} · {}", person.name, person.address)
        };
        people.push(
            div()
                .min_h(px(36.))
                .px_3()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(px(13.))
                        .child(shown),
                )
                .child(widgets::muted(
                    tr(if person.optional {
                        "ui.calendar_screen.optional"
                    } else {
                        "ui.calendar_screen.required"
                    }),
                    13.,
                    &theme,
                ))
                .child(
                    widgets::icon_button_sized(
                        ("cal-ed-remove", i),
                        Icon::MinusCircle,
                        24.,
                        14.,
                        false,
                        &theme,
                    )
                    .tooltip(widgets::tooltip(
                        trf("ui.calendar_screen.remove", &[("address", &person.address)]),
                        theme,
                    ))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        edit(this, cx, |f| {
                            if i < f.attendees.len() {
                                f.attendees.remove(i);
                            }
                        })
                    })),
                )
                .into_any_element(),
        );
    }

    let form_area = div()
        .id("cal-ed-scroll")
        .relative()
        .flex_1()
        .min_h_0()
        .overflow_y_scroll()
        .child(
            div()
                .flex()
                .flex_col()
                .gap_4()
                .pb_2()
                .child(group(first, &theme))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(
                            div()
                                .px_1()
                                .text_size(px(13.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(theme.text_soft)
                                .child(tr("ui.calendar_screen.attendees")),
                        )
                        .child(group(people, &theme)),
                ),
        )
        // Busy: the form takes no clicks until the core answers.
        .when(busy, |d| {
            d.child(
                div()
                    .absolute()
                    .inset_0()
                    .occlude()
                    .bg(theme.surface.opacity(0.4)),
            )
        });

    let save_enabled = can_save(this);
    let footer = div()
        .flex()
        .items_center()
        .gap_2()
        .child(widgets::disabled(
            widgets::button(
                "cal-ed-close",
                tr("ui.calendar_screen.close"),
                ButtonKind::Outline,
                &theme,
            )
            .when(!busy, |d| {
                d.on_click(cx.listener(|this, _, _, cx| close(this, cx)))
            }),
            busy,
        ))
        .child(div().flex_1())
        .when(busy, |d| {
            d.child(widgets::spinner("cal-ed-busy", 14., theme.muted))
        })
        .child(widgets::disabled(
            widgets::button(
                "cal-ed-save",
                tr(if editing {
                    "ui.calendar_screen.save"
                } else {
                    "ui.calendar_screen.create"
                }),
                ButtonKind::Primary,
                &theme,
            )
            .when(save_enabled, |d| {
                d.on_click(cx.listener(|this, _, _, cx| save(this, cx)))
            }),
            !save_enabled,
        ));

    // Inside the modal's own limit (86 % of the window), so the footer stays in view.
    let height = (window.viewport_size().height * 0.86 - px(4.)).min(px(720.));
    let content = div()
        .h(height)
        .p(px(24.))
        .flex()
        .flex_col()
        .gap(px(14.))
        .bg(theme.background)
        .child(
            div()
                .text_size(px(22.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(tr(if editing {
                    "ui.calendar_screen.edit_event"
                } else {
                    "ui.calendar_screen.new_event"
                })),
        )
        .when_some(model.error.clone(), |d, error| {
            d.child(
                div()
                    .text_size(px(12.))
                    .text_color(theme.error_text)
                    .child(error),
            )
        })
        .child(form_area)
        .child(footer);
    let entity = cx.entity().downgrade();
    Some(widgets::modal(
        "cal-editor",
        px(660.),
        content,
        &theme,
        move |_, cx| {
            let _ = entity.update(cx, |this, cx| close(this, cx));
        },
    ))
}

/// For `OXIDE_SCRIPT`: changes the open form as a click would.
pub fn debug_step(this: &mut Oxide, verb: &str, arg: &str, cx: &mut Context<Oxide>) -> bool {
    match verb {
        "picker" => {
            let picker = match arg {
                "start-date" => Some(Picker::StartDate),
                "start-time" => Some(Picker::StartTime),
                "end-date" => Some(Picker::EndDate),
                "end-time" => Some(Picker::EndTime),
                "repeat" => Some(Picker::Repeat),
                "unit" => Some(Picker::Unit),
                "series-end" => Some(Picker::SeriesEnd),
                _ => None,
            };
            set_picker(this, picker, cx);
        }
        "repeat" => {
            let repeat = match arg {
                "never" => Repeat::Never,
                "weekdays" => Repeat::Weekdays,
                "daily" => Repeat::Daily,
                "weekly" => Repeat::Weekly,
                "monthly" => Repeat::Monthly,
                "yearly" => Repeat::Yearly,
                _ => Repeat::Custom,
            };
            edit(this, cx, |f| set_repeat(f, repeat));
        }
        "unit" => {
            let unit = match arg {
                "day" => RepeatUnit::Day,
                "month" => RepeatUnit::Month,
                "year" => RepeatUnit::Year,
                _ => RepeatUnit::Week,
            };
            edit(this, cx, |f| f.recurrence.unit = unit);
        }
        "ends" => edit(this, cx, |f| {
            f.recurrence.end_date = Day::of(parse_iso(&f.start).unwrap_or_else(dates::now))
                .add_months(3)
                .iso()
        }),
        "all-day" => edit(this, cx, |f| {
            let all_day = !f.all_day;
            set_all_day(f, all_day)
        }),
        "title" => {
            let value = arg.to_owned();
            this.ui
                .calendar
                .editor
                .subject
                .update(cx, |i, cx| i.set_text(value.clone(), None, cx));
            edit(this, cx, |f| f.subject = value);
        }
        "query" => {
            let value = arg.to_owned();
            this.ui
                .calendar
                .editor
                .query
                .update(cx, |i, cx| i.set_text(value.clone(), None, cx));
            query_changed(this, value, cx);
        }
        "attendee" => add(
            this,
            CalendarPerson {
                address: arg.to_owned(),
                ..Default::default()
            },
            cx,
        ),
        "save" => save(this, cx),
        "close" => close(this, cx),
        _ => return false,
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moving_the_start_past_the_end_pushes_the_end() {
        let day = Day::from_ymd(2026, 11, 20);
        let mut form = CalendarDraft {
            start: iso(day.at(9, 0)),
            end: iso(day.at(9, 30)),
            ..Default::default()
        };
        set_field_time(&mut form, true, day.at(10, 0));
        assert_eq!(form.end, iso(day.at(10, 30)));
        // An earlier start keeps the end.
        set_field_time(&mut form, true, day.at(8, 0));
        assert_eq!(form.end, iso(day.at(10, 30)));
    }

    #[test]
    fn all_day_fields_show_the_last_day_and_store_the_next_midnight() {
        let day = Day::from_ymd(2026, 11, 20);
        let mut form = CalendarDraft {
            start: iso(day.at(9, 0)),
            end: iso(day.at(9, 30)),
            ..Default::default()
        };
        set_all_day(&mut form, true);
        assert_eq!(form.start, iso(day.start()));
        assert_eq!(form.end, iso(day.add(1).start()));
        assert_eq!(Day::of(field_time(&form, false)), day);
        set_field_time(&mut form, false, day.add(2).at(0, 0));
        assert_eq!(form.end, iso(day.add(3).start()));
    }

    #[test]
    fn a_custom_repeat_starts_weekly_on_the_start_day() {
        let day = Day::from_ymd(2026, 11, 20);
        let mut form = CalendarDraft {
            start: iso(day.at(9, 0)),
            end: iso(day.at(9, 30)),
            ..Default::default()
        };
        set_repeat(&mut form, Repeat::Custom);
        assert_eq!(form.recurrence.interval, 1);
        assert_eq!(form.recurrence.unit, RepeatUnit::Week);
        assert_eq!(form.recurrence.weekdays, vec![5]);
    }
}

//! The top bar's account button and its status card, and the status message dialog (StatusMenu.swift).
use crate::icons::{Icon, icon};
use crate::oxide::{Oxide, tr, trf};
use crate::settings::Page;
use crate::settings_controls::{self as controls, Choice};
use crate::theme::Theme;
use crate::widgets::{self, ButtonKind};
use gpui::{AnyElement, Context, Corner, FontWeight, SharedString, Window, div, prelude::*, px};
use teams_core::{
    Action, MeAction, MeView, MyStatus, Nav, PresenceStatus, PresenceView, StatusDuration,
};

const KEY: &str = "settings-account";

pub const STATUSES: [MyStatus; 6] = [
    MyStatus::Available,
    MyStatus::Busy,
    MyStatus::DoNotDisturb,
    MyStatus::BeRightBack,
    MyStatus::Away,
    MyStatus::Offline,
];
const DURATIONS: [StatusDuration; 7] = [
    StatusDuration::Never,
    StatusDuration::HalfHour,
    StatusDuration::OneHour,
    StatusDuration::TwoHours,
    StatusDuration::FourHours,
    StatusDuration::Today,
    StatusDuration::ThisWeek,
];

fn presence_of(status: MyStatus) -> PresenceStatus {
    match status {
        MyStatus::Available => PresenceStatus::Available,
        MyStatus::Busy => PresenceStatus::Busy,
        MyStatus::DoNotDisturb => PresenceStatus::DoNotDisturb,
        MyStatus::BeRightBack => PresenceStatus::BeRightBack,
        MyStatus::Away => PresenceStatus::Away,
        MyStatus::Offline => PresenceStatus::Offline,
    }
}

fn status_label(status: MyStatus) -> String {
    tr(match status {
        MyStatus::Available => "ui.presence.available",
        MyStatus::Busy => "ui.presence.busy",
        MyStatus::DoNotDisturb => "ui.presence.do_not_disturb",
        MyStatus::BeRightBack => "ui.presence.be_right_back",
        MyStatus::Away => "ui.status_menu.appear_away",
        MyStatus::Offline => "ui.status_menu.appear_offline",
    })
}

fn duration_label(duration: StatusDuration) -> String {
    tr(match duration {
        StatusDuration::HalfHour => "ui.status_menu.30_minutes",
        StatusDuration::OneHour => "ui.status_menu.1_hour",
        StatusDuration::TwoHours => "ui.status_menu.2_hours",
        StatusDuration::FourHours => "ui.status_menu.4_hours",
        StatusDuration::Today => "ui.status_menu.today",
        StatusDuration::ThisWeek => "ui.status_menu.this_week",
        StatusDuration::Never => "ui.status_menu.never",
    })
}

/// "Busy", or "Away, out of office": the presence badge's label.
pub fn presence_label(presence: &PresenceView) -> String {
    let status = tr(match presence.status {
        PresenceStatus::Available => "ui.presence.available",
        PresenceStatus::Busy => "ui.presence.busy",
        PresenceStatus::DoNotDisturb => "ui.presence.do_not_disturb",
        PresenceStatus::BeRightBack => "ui.presence.be_right_back",
        PresenceStatus::Away => "ui.presence.away",
        PresenceStatus::Offline => "ui.presence.offline",
        PresenceStatus::Unknown => "ui.presence.unknown",
    });
    if presence.out_of_office {
        trf("ui.presence.out_of_office", &[("status", &status)])
    } else {
        status
    }
}

/// The presence colour as a small disc; offline and unknown are a ring, as `PresenceBadge`.
fn badge(status: PresenceStatus, size: f32, theme: &Theme) -> AnyElement {
    let color = Theme::presence(status, false).unwrap_or(theme.subtle);
    let hollow = matches!(status, PresenceStatus::Offline | PresenceStatus::Unknown);
    div()
        .size(px(size))
        .flex_none()
        .rounded_full()
        .when(hollow, |d| d.border_2().border_color(color))
        .when(!hollow, |d| d.bg(color))
        .into_any_element()
}

pub fn open_card(this: &mut Oxide, cx: &mut Context<Oxide>) {
    if !this.popup_open(KEY) {
        this.send(Action::Me {
            action: MeAction::Refresh,
        });
        this.ui.settings.inline_open = None;
        this.popup = Some(KEY.into());
        cx.notify();
    }
}

pub fn account_button(
    this: &mut Oxide,
    _window: &mut Window,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    let Some(shell) = this.shell.clone() else {
        return div().into_any_element();
    };
    let theme = this.theme;
    let presence = this.me.as_ref().and_then(|m| m.presence.clone());
    let avatar = this.avatar(
        &shell.me,
        &shell.display_name,
        32.,
        teams_core::AvatarShape::Round,
        presence.as_ref(),
    );
    let hover = theme.input;
    let button = div()
        .id("account-button")
        .size(px(36.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(8.))
        .cursor_pointer()
        .hover(move |s| s.bg(hover))
        .child(avatar)
        .tooltip(widgets::tooltip(tr("ui.main_view.your_account"), theme))
        .on_click(cx.listener(|this, _, _, cx| {
            if this.popup_open(KEY) {
                this.close_popup(cx);
            } else if !controls_closed_just_now(this) {
                open_card(this, cx);
            }
        }));
    let mut container = div().relative().flex_none().child(button);
    if this.popup_open(KEY) {
        let content = card(
            this,
            &shell.me,
            &shell.display_name,
            &shell.account_email,
            cx,
        );
        let pop = controls::popover(this, KEY, Corner::TopRight, content, cx);
        container = container.child(div().absolute().top_full().right_0().child(pop));
    }
    container.into_any_element()
}

fn controls_closed_just_now(this: &Oxide) -> bool {
    this.ui
        .settings
        .just_closed
        .as_ref()
        .is_some_and(|(k, at)| k.as_ref() == KEY && at.elapsed().as_millis() < 400)
}

fn card(
    this: &mut Oxide,
    id: &str,
    name: &str,
    email: &str,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    let theme = this.theme;
    let me = this.me.clone().unwrap_or(MeView {
        note_limit: 2000,
        ..MeView::default()
    });
    let header = div()
        .flex()
        .items_center()
        .gap_3()
        .child(this.avatar(
            id,
            name,
            44.,
            teams_core::AvatarShape::Round,
            me.presence.as_ref(),
        ))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(3.))
                .child(
                    div()
                        .text_size(px(15.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(name.to_owned()),
                )
                .when(!email.is_empty(), |d| {
                    d.child(widgets::muted(email.to_owned(), 12., &theme))
                })
                .when_some(me.presence.as_ref(), |d, presence| {
                    let mut label = presence_label(presence);
                    if let Some(until) = &me.forced_until {
                        label = format!("{label} · {until}");
                    }
                    d.child(
                        div()
                            .text_size(px(12.))
                            .text_color(theme.text_soft)
                            .child(label),
                    )
                }),
        )
        .when(me.saving, |d| {
            d.child(widgets::spinner("status-saving", 14., theme.muted))
        });

    let ooo = me.out_of_office.then(|| {
        let text = if me.out_of_office_note.is_empty() {
            tr("ui.status_menu.automatic_replies_on")
        } else {
            me.out_of_office_note.clone()
        };
        div()
            .flex()
            .items_start()
            .gap_2()
            .text_size(px(12.))
            .text_color(theme.text_soft)
            .child(icon(Icon::Airplane, px(13.), theme.text_soft))
            .child(div().flex_1().line_clamp(3).child(text))
    });

    let hover = theme.input;
    let note = div()
        .id("status-note")
        .flex()
        .items_start()
        .gap(px(10.))
        .cursor_pointer()
        .rounded(px(6.))
        .hover(move |s| s.bg(hover))
        .child(icon(Icon::Chat, px(15.), theme.accent))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(
                    div()
                        .text_size(px(13.))
                        .text_color(theme.text)
                        .line_clamp(4)
                        .child(if me.note.is_empty() {
                            tr("ui.status_menu.set_status_message")
                        } else {
                            me.note.clone()
                        }),
                )
                .when_some(me.note_until.clone(), |d, until| {
                    d.child(widgets::muted(until, 11., &theme))
                }),
        )
        .child(icon(Icon::Pencil, px(13.), theme.subtle))
        .on_click(cx.listener(|this, _, window, cx| open_note(this, window, cx)));

    let mut statuses = div().flex().flex_col().gap(px(2.));
    for (ix, status) in STATUSES.into_iter().enumerate() {
        let current = me.forced
            && me
                .presence
                .as_ref()
                .is_some_and(|p| p.status == presence_of(status));
        statuses = statuses.child(
            div()
                .id(SharedString::from(format!("status-{ix}")))
                .min_h(px(28.))
                .px_2()
                .flex()
                .items_center()
                .gap(px(10.))
                .rounded(px(8.))
                .cursor_pointer()
                .hover(move |s| s.bg(hover))
                .child(badge(presence_of(status), 12., &theme))
                .child(
                    div()
                        .flex_1()
                        .text_size(px(13.))
                        .child(status_label(status)),
                )
                .when(current, |d| {
                    d.child(icon(Icon::Check, px(11.), theme.accent))
                })
                .on_click(cx.listener(move |this, _, _, _| {
                    let duration = this.ui.settings.status_duration;
                    this.send(Action::Me {
                        action: MeAction::SetStatus { status, duration },
                    });
                })),
        );
    }
    let chosen = this.ui.settings.status_duration;
    let durations = DURATIONS
        .into_iter()
        .map(|d| {
            Choice::new(duration_label(d), d == chosen, move |this, _, _| {
                this.ui.settings.status_duration = d
            })
        })
        .collect();
    let picker = controls::dropdown(this, "status-duration", 170., false, true, durations, cx);
    statuses = statuses.child(
        div()
            .pt_1()
            .flex()
            .items_start()
            .gap_2()
            .child(
                div()
                    .h(px(28.))
                    .flex()
                    .items_center()
                    .text_size(px(12.))
                    .text_color(theme.muted)
                    .child(tr("ui.status_menu.reset_after")),
            )
            .child(picker),
    );
    if me.forced {
        statuses = statuses.child(
            div().pt_1().flex().child(
                widgets::button(
                    "status-reset",
                    tr("ui.status_menu.reset_status"),
                    ButtonKind::Outline,
                    &theme,
                )
                .on_click(cx.listener(|this, _, _, _| {
                    this.send(Action::Me {
                        action: MeAction::ResetStatus,
                    })
                })),
            ),
        );
    }

    let plain = |id: &'static str, glyph: Icon, label: String| {
        div()
            .id(id)
            .flex()
            .items_center()
            .gap_2()
            .text_size(px(13.))
            .cursor_pointer()
            .child(icon(glyph, px(14.), theme.text_soft))
            .child(label)
    };
    let out_of_office = plain(
        "status-out-of-office",
        Icon::Airplane,
        tr("ui.status_menu.schedule_out_of_office"),
    )
    .on_click(cx.listener(|this, _, _, cx| {
        this.close_popup(cx);
        crate::settings::open(this, Page::OutOfOffice, cx);
    }));
    let settings = plain("status-settings", Icon::Gear, tr("ui.status_menu.settings")).on_click(
        cx.listener(|this, _, _, cx| {
            this.close_popup(cx);
            this.send(Action::Navigate { nav: Nav::Settings });
        }),
    );
    let error = me.error.clone().map(|error| {
        controls::error_line("status-error-close", error, &theme, cx, |this| {
            this.send(Action::Me {
                action: MeAction::DismissError,
            })
        })
    });

    div()
        .w(px(320.))
        .p_4()
        .flex()
        .flex_col()
        .gap(px(14.))
        .child(header)
        .children(ooo)
        .child(widgets::divider(&theme))
        .child(note)
        .child(widgets::divider(&theme))
        .child(statuses)
        .child(widgets::divider(&theme))
        .child(out_of_office)
        .child(settings)
        .children(error)
        .into_any_element()
}

pub fn open_note(this: &mut Oxide, window: &mut Window, cx: &mut Context<Oxide>) {
    let note = this.me.as_ref().map(|m| m.note.clone()).unwrap_or_default();
    this.close_popup(cx);
    this.ui.settings.inline_open = None;
    this.ui.settings.note_sheet = true;
    this.ui.settings.note_duration = StatusDuration::Never;
    let input = this.ui.settings.note_input.clone();
    input.update(cx, |i, cx| {
        let end = note.len();
        i.set_text(note, Some(end), cx);
    });
    input.read(cx).focus(window);
    cx.notify();
}

/// "Done": a blank message clears it.
pub fn save_note(this: &mut Oxide, cx: &mut Context<Oxide>) {
    let text = this
        .ui
        .settings
        .note_input
        .read(cx)
        .text()
        .trim()
        .to_owned();
    let duration = this.ui.settings.note_duration;
    this.send(Action::Me {
        action: if text.is_empty() {
            MeAction::ClearNote
        } else {
            MeAction::SetNote { text, duration }
        },
    });
    this.ui.settings.note_sheet = false;
    this.ui.settings.inline_open = None;
    cx.notify();
}

/// Writes or clears the status message, as the web client's "Set status message" dialog.
pub fn note_sheet(
    this: &mut Oxide,
    _window: &mut Window,
    cx: &mut Context<Oxide>,
) -> Option<AnyElement> {
    if !this.ui.settings.note_sheet {
        return None;
    }
    let theme = this.theme;
    let me = this.me.clone().unwrap_or(MeView {
        note_limit: 2000,
        ..MeView::default()
    });
    let input = this.ui.settings.note_input.clone();
    let count = input.read(cx).text().chars().count();
    let chosen = this.ui.settings.note_duration;
    let durations = DURATIONS
        .into_iter()
        .map(|d| {
            Choice::new(duration_label(d), d == chosen, move |this, _, _| {
                this.ui.settings.note_duration = d
            })
        })
        .collect();
    let picker = controls::dropdown(this, "note-duration", 180., false, true, durations, cx);
    let field = widgets::text_field(&input, &theme, cx)
        .min_h(px(90.))
        .border_color(theme.border_strong);
    let content = div()
        .p(px(20.))
        .flex()
        .flex_col()
        .gap(px(14.))
        .child(
            div()
                .text_size(px(17.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(tr("ui.status_menu.set_status_message")),
        )
        .child(field)
        .child(
            div()
                .flex()
                .items_start()
                .gap_2()
                .child(div().h(px(28.)).flex().items_center().child(widgets::muted(
                    tr("ui.status_menu.clear_after"),
                    12.,
                    &theme,
                )))
                .child(picker)
                .child(div().flex_1())
                .child(div().h(px(28.)).flex().items_center().child(widgets::muted(
                    format!("{count}/{}", me.note_limit),
                    11.,
                    &theme,
                ))),
        )
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .when(!me.note.is_empty(), |d| {
                    d.child(
                        widgets::button(
                            "note-clear",
                            tr("ui.status_menu.clear_message"),
                            ButtonKind::Outline,
                            &theme,
                        )
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.send(Action::Me {
                                action: MeAction::ClearNote,
                            });
                            this.ui.settings.note_sheet = false;
                            cx.notify();
                        })),
                    )
                })
                .child(div().flex_1())
                .child(
                    widgets::button(
                        "note-cancel",
                        tr("ui.status_menu.cancel"),
                        ButtonKind::Outline,
                        &theme,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.ui.settings.note_sheet = false;
                        cx.notify();
                    })),
                )
                .child(
                    widgets::button(
                        "note-done",
                        tr("ui.status_menu.done"),
                        ButtonKind::Primary,
                        &theme,
                    )
                    .on_click(cx.listener(|this, _, _, cx| save_note(this, cx))),
                ),
        );
    let entity = cx.entity().downgrade();
    Some(widgets::modal(
        "note-sheet",
        px(420.),
        content,
        &theme,
        move |_, cx| {
            let _ = entity.update(cx, |this, cx| {
                this.ui.settings.note_sheet = false;
                cx.notify();
            });
        },
    ))
}

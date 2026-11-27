//! Settings › Notifications and activity, the "pause until" dialog and the per-chat / per-channel notification
//! dialog (Notifications/NotificationSettingsView.swift). The core writes every change to the account.
use crate::icons::{Icon, icon};
use crate::oxide::{Oxide, tr, trf};
use crate::settings_controls::{
    self as controls, Choice, card, card_title, heading, labelled, switch_row,
};
use crate::theme::Theme;
use crate::widgets::{self, ButtonKind};
use gpui::{AnyElement, Context, Div, FontWeight, SharedString, div, prelude::*, px};
use teams_core::{
    Action, ChannelChoice, ChatDesktop, EmailDelay, MeetingChats, NotificationChoice,
    NotificationEditorView, NotificationPermission, NotificationPreset, NotificationRowView,
    NotificationSection, NotificationSound, NotificationToggle, NotificationsAction,
    NotificationsView,
};

fn send(this: &Oxide, action: NotificationsAction) {
    this.send(Action::Notifications { action });
}

pub fn page(this: &mut Oxide, cx: &mut Context<Oxide>) -> AnyElement {
    let title = tr("ui.notification_settings_view.notifications_and_activity");
    let Some(view) = this.notifications.clone() else {
        let theme = this.theme;
        let loading = widgets::spinner("notifications-loading", 16., theme.muted);
        return crate::settings::subpage(this, title, vec![loading], cx);
    };
    let theme = this.theme;
    let mut sections: Vec<AnyElement> = Vec::new();
    if let Some(error) = &view.error {
        sections.push(
            div()
                .text_size(px(12.))
                .text_color(theme.danger)
                .child(error.clone())
                .into_any_element(),
        );
    }
    sections.push(permission_card(&view, &theme, cx));
    sections.push(pause_card(this, &view, cx));
    sections.push(preset_card(&view, &theme, cx));
    let chats: Vec<_> = view
        .rows
        .iter()
        .filter(|r| r.section == NotificationSection::Chats)
        .cloned()
        .collect();
    sections.push(section(
        this,
        tr("ui.notification_settings_view.chats"),
        &chats,
        tr("ui.notification_settings_view.a_muted_chat_no_longer_shows"),
        cx,
    ));
    let channels: Vec<_> = view
        .rows
        .iter()
        .filter(|r| r.section == NotificationSection::TeamsAndChannels)
        .cloned()
        .collect();
    sections.push(section(
        this,
        tr("ui.notification_settings_view.teams_and_channels"),
        &channels,
        tr("ui.notification_settings_view.each_channel_also_has_its_own"),
        cx,
    ));
    sections.push(meetings_card(this, &view, cx));
    sections.push(sounds_card(this, &view, cx));
    sections.push(read_receipts_card(&view, &theme, cx));
    sections.push(email_card(this, &view, cx));
    if !view.realtime.is_empty() {
        sections.push(
            div()
                .flex()
                .items_center()
                .gap_1p5()
                .text_size(px(12.))
                .text_color(theme.muted)
                .child(icon(Icon::Antenna, px(13.), theme.muted))
                .child(view.realtime.clone())
                .into_any_element(),
        );
    }
    crate::settings::subpage(this, title, sections, cx)
}

fn permission_card(view: &NotificationsView, theme: &Theme, cx: &mut Context<Oxide>) -> AnyElement {
    let granted = view.permission == NotificationPermission::Granted;
    let (title, detail) = match view.permission {
        NotificationPermission::Granted => (
            "ui.notification_settings_view.macos_notifications_allowed",
            "ui.notification_settings_view.banner_style_and_focus_are_set",
        ),
        NotificationPermission::Denied => (
            "ui.notification_settings_view.macos_notifications_denied",
            "ui.notification_settings_view.no_banners_can_be_shown_while",
        ),
        NotificationPermission::Unknown => (
            "ui.notification_settings_view.macos_notifications_not_set_up",
            "ui.notification_settings_view.run_the_packaged_app_oxide_app",
        ),
    };
    let button = if granted {
        widgets::button(
            "notifications-test",
            tr("ui.notification_settings_view.test"),
            ButtonKind::Outline,
            theme,
        )
        .on_click(cx.listener(|this, _, _, _| send(this, NotificationsAction::SendTest)))
    } else {
        widgets::button(
            "notifications-system-settings",
            tr("ui.notification_settings_view.open_system_settings"),
            ButtonKind::Outline,
            theme,
        )
        .on_click(|_, _, cx| {
            cx.open_url("x-apple.systempreferences:com.apple.Notifications-Settings.extension")
        })
    };
    card(theme)
        .flex_row()
        .items_center()
        .gap_3()
        .child(icon(
            if granted { Icon::Bell } else { Icon::BellOff },
            px(19.),
            if granted { theme.accent } else { theme.danger },
        ))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(3.))
                .child(card_title(tr(title)))
                .child(widgets::muted(tr(detail), 12., theme)),
        )
        .child(button)
        .into_any_element()
}

fn pause_card(this: &mut Oxide, view: &NotificationsView, cx: &mut Context<Oxide>) -> AnyElement {
    let theme = this.theme;
    let title = match &view.paused_until {
        Some(until) => trf(
            "ui.notification_settings_view.banners_paused_until",
            &[("until", until)],
        ),
        None => tr("ui.notification_settings_view.pause_banners"),
    };
    let control = if view.paused_until.is_some() {
        widgets::button(
            "notifications-resume",
            tr("ui.notification_settings_view.resume"),
            ButtonKind::Outline,
            &theme,
        )
        .on_click(cx.listener(|this, _, _, _| send(this, NotificationsAction::Resume)))
        .into_any_element()
    } else {
        pause_menu(this, cx)
    };
    card(&theme)
        .flex_row()
        .items_center()
        .gap_3()
        .child(icon(Icon::Moon, px(17.), theme.muted))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(3.))
                .child(card_title(title))
                .child(widgets::muted(
                    tr("ui.notification_settings_view.the_activity_feed_badges_and_emails"),
                    12.,
                    &theme,
                )),
        )
        .child(control)
        .into_any_element()
}

const PAUSE_KEY: &str = "notifications-pause";

/// "Pause ▾": a menu of durations, then "Date and time…".
fn pause_menu(this: &mut Oxide, cx: &mut Context<Oxide>) -> AnyElement {
    let theme = this.theme;
    let trigger = widgets::button(
        "notifications-pause-trigger",
        tr("ui.notification_settings_view.pause"),
        ButtonKind::Outline,
        &theme,
    )
    .child(icon(Icon::ChevronDown, px(12.), theme.muted))
    .on_click(cx.listener(|this, _, _, cx| controls::toggle(this, PAUSE_KEY, cx)));
    let mut container = div().relative().flex_none().child(trigger);
    if this.popup_open(PAUSE_KEY) {
        let items: [(&str, NotificationsAction); 4] = [
            (
                "ui.notification_settings_view.10_minutes",
                NotificationsAction::Pause { minutes: 10 },
            ),
            (
                "ui.notification_settings_view.30_minutes",
                NotificationsAction::Pause { minutes: 30 },
            ),
            (
                "ui.notification_settings_view.1_hour",
                NotificationsAction::Pause { minutes: 60 },
            ),
            (
                "ui.notification_settings_view.until_tomorrow_8_00_am",
                NotificationsAction::PauseUntilTomorrow,
            ),
        ];
        let mut menu = div().flex().flex_col();
        for (ix, (key, action)) in items.into_iter().enumerate() {
            menu = menu.child(
                widgets::menu_item(
                    SharedString::from(format!("pause-{ix}")),
                    None,
                    tr(key),
                    &theme,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.close_popup(cx);
                    send(this, action.clone());
                })),
            );
        }
        menu = menu
            .child(div().my_1().child(widgets::divider(&theme)))
            .child(
                widgets::menu_item(
                    "pause-custom",
                    None,
                    tr("ui.notification_settings_view.date_and_time"),
                    &theme,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.close_popup(cx);
                    open_pause(this);
                    cx.notify();
                })),
            );
        let pop = controls::popover(this, PAUSE_KEY, gpui::Corner::TopRight, menu, cx);
        container = container.child(div().absolute().top_full().right_0().child(pop));
    }
    container.into_any_element()
}

fn preset_card(view: &NotificationsView, theme: &Theme, cx: &mut Context<Oxide>) -> AnyElement {
    let options = vec![
        (
            NotificationPreset::AllActivity,
            tr("ui.notification_settings_view.all_activity"),
        ),
        (
            NotificationPreset::MyAtMentions,
            tr("ui.notification_settings_view.mentions_and_replies"),
        ),
        (
            NotificationPreset::Custom,
            tr("ui.notification_settings_view.custom"),
        ),
    ];
    div()
        .flex()
        .flex_col()
        .gap(px(10.))
        .child(heading(tr("ui.notification_settings_view.preset")))
        .child(div().flex().child(controls::segmented(
            "preset",
            options,
            view.preset,
            theme,
            cx,
            |this, preset, _, _| send(this, NotificationsAction::SetPreset { preset }),
        )))
        .child(widgets::muted(
            tr("ui.notification_settings_view.a_preset_rewrites_every_row_below"),
            12.,
            theme,
        ))
        .into_any_element()
}

/// Rows with a divider between them, in a card.
fn rows_card(theme: &Theme, rows: Vec<AnyElement>) -> Div {
    let mut body = card(theme).gap_3();
    for (ix, row) in rows.into_iter().enumerate() {
        if ix > 0 {
            body = body.child(widgets::divider(theme));
        }
        body = body.child(row);
    }
    body
}

fn section(
    this: &mut Oxide,
    title: String,
    rows: &[NotificationRowView],
    footer: String,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    let theme = this.theme;
    let rows: Vec<AnyElement> = rows.iter().map(|row| choice_row(this, row, cx)).collect();
    div()
        .flex()
        .flex_col()
        .gap(px(10.))
        .child(heading(title))
        .child(rows_card(&theme, rows))
        .child(widgets::muted(footer, 12., &theme))
        .into_any_element()
}

fn choice_row(this: &mut Oxide, row: &NotificationRowView, cx: &mut Context<Oxide>) -> AnyElement {
    let key = format!("notification-row-{:?}", row.row);
    let which = row.row;
    let choices = [
        (
            NotificationChoice::BannerAndFeed,
            "ui.notification_settings_view.banner_and_feed",
        ),
        (
            NotificationChoice::OnlyFeed,
            "ui.notification_settings_view.only_show_in_feed",
        ),
        (NotificationChoice::Off, "ui.notification_settings_view.off"),
    ]
    .into_iter()
    .map(|(choice, label)| {
        Choice::new(tr(label), choice == row.choice, move |this, _, _| {
            send(this, NotificationsAction::SetChoice { row: which, choice })
        })
    })
    .collect();
    let picker = controls::dropdown(this, &key, 190., false, false, choices, cx);
    labelled(row.title.clone(), picker).into_any_element()
}

fn toggle(
    id: &str,
    title: String,
    on: bool,
    which: NotificationToggle,
    disabled: bool,
    theme: &Theme,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    switch_row(
        id,
        title,
        None,
        on,
        disabled,
        theme,
        cx,
        move |this, on, _| send(this, NotificationsAction::SetToggle { toggle: which, on }),
    )
    .into_any_element()
}

fn meetings_card(
    this: &mut Oxide,
    view: &NotificationsView,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    let theme = this.theme;
    let mut rows: Vec<AnyElement> = view
        .rows
        .iter()
        .filter(|r| r.section == NotificationSection::Meetings)
        .map(|row| choice_row(this, row, cx))
        .collect();
    let current = view.meeting_chats;
    let choices = [
        (
            MeetingChats::All,
            "ui.notification_settings_view.always_notify",
        ),
        (
            MeetingChats::Participated,
            "ui.notification_settings_view.mute_until_i_join_or_send",
        ),
        (MeetingChats::None, "ui.notification_settings_view.mute"),
    ]
    .into_iter()
    .map(|(choice, label)| {
        Choice::new(tr(label), choice == current, move |this, _, _| {
            send(this, NotificationsAction::SetMeetingChats { choice })
        })
    })
    .collect();
    let picker = controls::dropdown(
        this,
        "notification-meeting-chats",
        250.,
        false,
        false,
        choices,
        cx,
    );
    rows.push(
        labelled(tr("ui.notification_settings_view.meeting_chats"), picker).into_any_element(),
    );
    rows.push(toggle(
        "toggle-mute-meetings",
        tr("ui.notification_settings_view.mute_notifications_during_meetings_and_calls"),
        view.mute_during_meetings,
        NotificationToggle::MuteDuringMeetings,
        false,
        &theme,
        cx,
    ));
    div()
        .flex()
        .flex_col()
        .gap(px(10.))
        .child(heading(tr("ui.notification_settings_view.meetings")))
        .child(rows_card(&theme, rows))
        .into_any_element()
}

/// macOS system sounds standing in for Teams' ones, as `NotificationSounds.systemName`.
fn system_sound(sound: NotificationSound) -> &'static str {
    match sound {
        NotificationSound::Default => "Glass",
        NotificationSound::Vibe => "Purr",
        NotificationSound::Nudge => "Pop",
        NotificationSound::Tap => "Tink",
        NotificationSound::Flick => "Morse",
        NotificationSound::Pluck => "Bottle",
        NotificationSound::Summon => "Hero",
        NotificationSound::Ping => "Ping",
        NotificationSound::DefaultUrgent => "Sosumi",
        NotificationSound::Prioritize => "Blow",
        NotificationSound::Escalate => "Funk",
        NotificationSound::Alarm => "Submarine",
    }
}

fn sound_label(sound: NotificationSound) -> String {
    match sound {
        NotificationSound::Default => tr("ui.notification_router.default"),
        NotificationSound::Vibe => "Vibe".to_owned(),
        NotificationSound::Nudge => tr("ui.notification_router.nudge"),
        NotificationSound::Tap => tr("ui.notification_router.tap"),
        NotificationSound::Flick => tr("ui.notification_router.flick"),
        NotificationSound::Pluck => tr("ui.notification_router.pluck"),
        NotificationSound::Summon => tr("ui.notification_router.summon"),
        NotificationSound::Ping => "Ping".to_owned(),
        NotificationSound::DefaultUrgent => tr("ui.notification_router.default_urgent"),
        NotificationSound::Prioritize => tr("ui.notification_router.prioritize"),
        NotificationSound::Escalate => tr("ui.notification_router.escalate"),
        NotificationSound::Alarm => tr("ui.notification_router.alarm"),
    }
}

/// Plays a sample, as the macOS app does when a sound is picked.
pub fn play_sound(sound: NotificationSound) {
    let path = format!("/System/Library/Sounds/{}.aiff", system_sound(sound));
    let _ = std::process::Command::new("afplay").arg(path).spawn();
}

const STANDARD: [NotificationSound; 8] = [
    NotificationSound::Default,
    NotificationSound::Vibe,
    NotificationSound::Nudge,
    NotificationSound::Tap,
    NotificationSound::Flick,
    NotificationSound::Pluck,
    NotificationSound::Summon,
    NotificationSound::Ping,
];
const URGENT: [NotificationSound; 4] = [
    NotificationSound::DefaultUrgent,
    NotificationSound::Prioritize,
    NotificationSound::Escalate,
    NotificationSound::Alarm,
];

fn sound_picker(
    this: &mut Oxide,
    title: String,
    selection: NotificationSound,
    urgent: bool,
    disabled: bool,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    let theme = this.theme;
    let options: &[NotificationSound] = if urgent { &URGENT } else { &STANDARD };
    let choices = options
        .iter()
        .map(|&sound| {
            Choice::new(sound_label(sound), sound == selection, move |this, _, _| {
                play_sound(sound);
                send(this, NotificationsAction::SetSound { urgent, sound });
            })
        })
        .collect();
    let key = if urgent {
        "notification-urgent-sound"
    } else {
        "notification-sound"
    };
    let picker = controls::dropdown(this, key, 190., disabled, false, choices, cx);
    div()
        .flex()
        .items_center()
        .gap_3()
        .when(disabled, |d| d.opacity(0.45))
        .child(div().flex_1().min_w_0().text_size(px(13.)).child(title))
        .child(picker)
        .child(
            widgets::icon_button_sized(
                SharedString::from(format!("{key}-play")),
                Icon::Speaker,
                28.,
                13.,
                false,
                &theme,
            )
            .tooltip(widgets::tooltip(
                tr("ui.notification_settings_view.play"),
                theme,
            ))
            .on_click(move |_, _, _| {
                if !disabled {
                    play_sound(selection)
                }
            }),
        )
        .into_any_element()
}

fn sounds_card(this: &mut Oxide, view: &NotificationsView, cx: &mut Context<Oxide>) -> AnyElement {
    let theme = this.theme;
    let off = !view.sounds;
    let sounds = div()
        .flex()
        .flex_col()
        .gap_3()
        .child(toggle(
            "toggle-sounds",
            tr("ui.notification_settings_view.play_sound_for_notifications"),
            view.sounds,
            NotificationToggle::Sounds,
            false,
            &theme,
            cx,
        ))
        .child(sound_picker(
            this,
            tr("ui.notification_settings_view.sound"),
            view.sound,
            false,
            off,
            cx,
        ))
        .child(sound_picker(
            this,
            tr("ui.notification_settings_view.urgent_sound"),
            view.urgent_sound,
            true,
            off,
            cx,
        ))
        .child(toggle(
            "toggle-sounds-busy",
            tr("ui.notification_settings_view.sounds_when_i_m_busy_or"),
            view.sounds_when_busy,
            NotificationToggle::SoundsWhenBusy,
            off,
            &theme,
            cx,
        ))
        .child(toggle(
            "toggle-sounds-urgent",
            tr("ui.notification_settings_view.sounds_for_urgent_and_priority_messages"),
            view.sounds_for_urgent,
            NotificationToggle::SoundsForUrgent,
            off,
            &theme,
            cx,
        ));
    let rows = vec![
        toggle(
            "toggle-preview",
            tr("ui.notification_settings_view.show_message_preview"),
            view.show_preview,
            NotificationToggle::ShowPreview,
            false,
            &theme,
            cx,
        ),
        sounds.into_any_element(),
        toggle(
            "toggle-call-sounds",
            tr("ui.notification_settings_view.ring_for_incoming_calls"),
            view.call_sounds,
            NotificationToggle::CallSounds,
            false,
            &theme,
            cx,
        ),
        toggle(
            "toggle-only-urgent",
            tr("ui.notification_settings_view.only_urgent_and_priority_notifications"),
            view.only_urgent,
            NotificationToggle::OnlyUrgent,
            false,
            &theme,
            cx,
        ),
    ];
    div()
        .flex()
        .flex_col()
        .gap(px(10.))
        .child(heading(tr(
            "ui.notification_settings_view.sounds_and_preview",
        )))
        .child(rows_card(&theme, rows))
        .into_any_element()
}

fn read_receipts_card(
    view: &NotificationsView,
    theme: &Theme,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    let row = switch_row(
        "toggle-read-receipts",
        tr("ui.notification_settings_view.read_receipts"),
        None,
        view.read_receipts,
        false,
        theme,
        cx,
        |this, on, _| send(this, NotificationsAction::SetReadReceipts { on }),
    );
    div()
        .flex()
        .flex_col()
        .gap(px(10.))
        .child(heading(tr("ui.notification_settings_view.privacy")))
        .child(card(theme).gap_2().child(row).child(widgets::muted(
            tr("ui.notification_settings_view.read_receipts_detail"),
            12.,
            theme,
        )))
        .into_any_element()
}

fn email_card(this: &mut Oxide, view: &NotificationsView, cx: &mut Context<Oxide>) -> AnyElement {
    let theme = this.theme;
    let current = view.email_delay;
    let choices = [
        (
            EmailDelay::Immediately,
            "ui.notification_settings_view.as_soon_as_possible",
        ),
        (
            EmailDelay::Every10Mins,
            "ui.notification_settings_view.every_10_minutes",
        ),
        (EmailDelay::Hourly, "ui.notification_settings_view.hourly"),
        (
            EmailDelay::EveryEightHours,
            "ui.notification_settings_view.every_8_hours",
        ),
        (EmailDelay::Daily, "ui.notification_settings_view.daily"),
        (EmailDelay::Never, "ui.notification_settings_view.never"),
    ]
    .into_iter()
    .map(|(delay, label)| {
        Choice::new(tr(label), delay == current, move |this, _, _| {
            send(this, NotificationsAction::SetEmailDelay { delay })
        })
    })
    .collect();
    let picker = controls::dropdown(this, "notification-email", 210., false, false, choices, cx);
    div()
        .flex()
        .flex_col()
        .gap(px(10.))
        .child(heading(tr("ui.notification_settings_view.email")))
        .child(card(&theme).child(labelled(
            tr("ui.notification_settings_view.missed_activity_emails"),
            picker,
        )))
        .into_any_element()
}

/// Opens "Pause banners until", two hours from now.
pub fn open_pause(this: &mut Oxide) {
    let now_ms = crate::settings_time::now() * 1000;
    let step = 30 * 60 * 1000;
    this.ui.settings.pause_until = Some((now_ms + 2 * 3600 * 1000 + step / 2) / step * step);
}

pub fn pause_sheet(this: &mut Oxide, cx: &mut Context<Oxide>) -> Option<AnyElement> {
    let until = this.ui.settings.pause_until?;
    let theme = this.theme;
    let now_ms = crate::settings_time::now() * 1000;
    let picker = controls::date_time_stepper(
        "pause-until",
        until,
        Some(now_ms),
        &theme,
        cx,
        |this, ms| this.ui.settings.pause_until = Some(ms),
    );
    let content = div()
        .p(px(24.))
        .flex()
        .flex_col()
        .gap_4()
        .child(heading(tr(
            "ui.notification_settings_view.pause_banners_until",
        )))
        .child(picker)
        .child(
            div()
                .flex()
                .gap_2()
                .child(div().flex_1())
                .child(
                    widgets::button(
                        "pause-cancel",
                        tr("ui.notification_settings_view.cancel"),
                        ButtonKind::Outline,
                        &theme,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.ui.settings.pause_until = None;
                        cx.notify();
                    })),
                )
                .child(
                    widgets::button(
                        "pause-confirm",
                        tr("ui.notification_settings_view.pause"),
                        ButtonKind::Primary,
                        &theme,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        send(this, NotificationsAction::PauseUntil { epoch_ms: until });
                        this.ui.settings.pause_until = None;
                        cx.notify();
                    })),
                ),
        );
    let entity = cx.entity().downgrade();
    Some(widgets::modal(
        "pause-sheet",
        px(400.),
        content,
        &theme,
        move |_, cx| {
            let _ = entity.update(cx, |this, cx| {
                this.ui.settings.pause_until = None;
                cx.notify();
            });
        },
    ))
}

/// The dialog's edits, applied by Save.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EditorDraft {
    muted: bool,
    everyone: bool,
    desktop: ChatDesktop,
    posts: ChannelChoice,
    replies: bool,
    mentions: ChannelChoice,
    follow_all: bool,
}
impl EditorDraft {
    fn of(editor: &NotificationEditorView) -> Self {
        Self {
            muted: editor.muted,
            everyone: editor.everyone_mentions,
            desktop: editor.desktop,
            posts: editor.all_new_posts,
            replies: editor.include_replies,
            mentions: editor.channel_mentions,
            follow_all: editor.follow_all_threads,
        }
    }
}

fn edit(this: &mut Oxide, change: impl FnOnce(&mut EditorDraft)) {
    if let Some(draft) = this.ui.settings.editor_draft.as_mut() {
        change(draft);
    }
}

/// The settings of one chat or one channel (`NotificationsView.editor`).
pub fn editor_sheet(this: &mut Oxide, cx: &mut Context<Oxide>) -> Option<AnyElement> {
    let Some(editor) = this.notifications.as_ref().and_then(|n| n.editor.clone()) else {
        this.ui.settings.editor_draft = None;
        return None;
    };
    // The form starts from what the service holds, once read.
    if editor.loading {
        this.ui.settings.editor_draft = None;
    } else if this.ui.settings.editor_draft.is_none() {
        this.ui.settings.editor_draft = Some(EditorDraft::of(&editor));
    }
    let theme = this.theme;
    let draft = this.ui.settings.editor_draft.clone();
    let form = match &draft {
        _ if editor.loading => Some(
            div()
                .flex()
                .justify_center()
                .child(widgets::spinner("editor-loading", 16., theme.muted))
                .into_any_element(),
        ),
        Some(draft) if editor.channel => Some(channel_form(this, draft, cx)),
        Some(draft) => Some(chat_form(this, draft, cx)),
        None => None,
    };
    let unchanged = draft
        .as_ref()
        .is_none_or(|d| *d == EditorDraft::of(&editor));
    let save_disabled = unchanged || editor.loading || editor.saving;
    let channel = editor.channel;
    let content = div()
        .p(px(24.))
        .flex()
        .flex_col()
        .gap_4()
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(3.))
                .child(
                    div()
                        .text_size(px(18.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(tr("ui.notification_settings_view.notifications")),
                )
                .child(
                    widgets::muted(editor.title.clone(), 13., &theme)
                        .overflow_hidden()
                        .whitespace_nowrap()
                        .text_ellipsis(),
                ),
        )
        .children(form)
        .when_some(editor.error.clone(), |d, error| {
            d.child(
                div()
                    .text_size(px(12.))
                    .text_color(theme.danger)
                    .child(error),
            )
        })
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .when(editor.saving, |d| {
                    d.child(widgets::spinner("editor-saving", 14., theme.muted))
                })
                .child(div().flex_1())
                .child(
                    widgets::button(
                        "editor-close",
                        tr("ui.notification_settings_view.close"),
                        ButtonKind::Outline,
                        &theme,
                    )
                    .on_click(
                        cx.listener(|this, _, _, _| send(this, NotificationsAction::CloseEditor)),
                    ),
                )
                .child(
                    widgets::disabled(
                        widgets::button(
                            "editor-save",
                            tr("ui.notification_settings_view.save"),
                            ButtonKind::Primary,
                            &theme,
                        ),
                        save_disabled,
                    )
                    .on_click(cx.listener(move |this, _, _, _| {
                        if save_disabled {
                            return;
                        }
                        let Some(d) = this.ui.settings.editor_draft.clone() else {
                            return;
                        };
                        let action = if channel {
                            NotificationsAction::SetChannelSettings {
                                all_new_posts: d.posts,
                                include_replies: d.replies,
                                channel_mentions: d.mentions,
                                follow_all_threads: d.follow_all,
                            }
                        } else {
                            NotificationsAction::SetChatSettings {
                                muted: d.muted,
                                everyone_mentions: d.everyone,
                                desktop: d.desktop,
                            }
                        };
                        send(this, action);
                    })),
                ),
        );
    let entity = cx.entity().downgrade();
    Some(widgets::modal(
        "notification-editor",
        px(440.),
        content,
        &theme,
        move |_, cx| {
            let _ = entity.update(cx, |this, _| send(this, NotificationsAction::CloseEditor));
        },
    ))
}

fn chat_form(this: &mut Oxide, draft: &EditorDraft, cx: &mut Context<Oxide>) -> AnyElement {
    let theme = this.theme;
    let desktop = draft.desktop;
    let choices = [
        (
            ChatDesktop::FollowGlobal,
            "ui.notification_settings_view.follow_general_settings",
        ),
        (ChatDesktop::On, "ui.notification_settings_view.always_show"),
        (ChatDesktop::Off, "ui.notification_settings_view.never_show"),
    ]
    .into_iter()
    .map(|(value, label)| {
        Choice::new(tr(label), value == desktop, move |this, _, _| {
            edit(this, |d| d.desktop = value)
        })
    })
    .collect();
    let picker = controls::dropdown(this, "editor-desktop", 210., false, true, choices, cx);
    div()
        .flex()
        .flex_col()
        .gap_3()
        .child(switch_row(
            "editor-muted",
            tr("ui.notification_settings_view.mute_chat"),
            Some(tr(
                "ui.notification_settings_view.mentions_and_urgent_messages_still_show",
            )),
            draft.muted,
            false,
            &theme,
            cx,
            |this, on, _| edit(this, |d| d.muted = on),
        ))
        .child(switch_row(
            "editor-everyone",
            tr("ui.notification_settings_view.everyone_mentions_everyone"),
            None,
            draft.everyone,
            draft.muted,
            &theme,
            cx,
            |this, on, _| edit(this, |d| d.everyone = on),
        ))
        .child(
            div()
                .flex()
                .items_start()
                .gap_3()
                .child(
                    div()
                        .flex_1()
                        .h(px(28.))
                        .flex()
                        .items_center()
                        .text_size(px(13.))
                        .child(tr("ui.notification_settings_view.banners")),
                )
                .child(picker),
        )
        .into_any_element()
}

fn channel_picker(
    this: &mut Oxide,
    key: &str,
    title: String,
    current: ChannelChoice,
    set: fn(&mut EditorDraft, ChannelChoice),
    cx: &mut Context<Oxide>,
) -> Div {
    let choices = [
        (
            ChannelChoice::BannerAndFeed,
            "ui.notification_settings_view.banner_and_feed",
        ),
        (
            ChannelChoice::Feed,
            "ui.notification_settings_view.only_show_in_feed",
        ),
        (ChannelChoice::Off, "ui.notification_settings_view.off"),
    ]
    .into_iter()
    .map(|(value, label)| {
        Choice::new(tr(label), value == current, move |this, _, _| {
            edit(this, |d| set(d, value))
        })
    })
    .collect();
    let picker = controls::dropdown(this, key, 190., false, true, choices, cx);
    div()
        .flex()
        .items_start()
        .gap_3()
        .child(
            div()
                .flex_1()
                .h(px(28.))
                .flex()
                .items_center()
                .text_size(px(13.))
                .child(title),
        )
        .child(picker)
}

fn channel_form(this: &mut Oxide, draft: &EditorDraft, cx: &mut Context<Oxide>) -> AnyElement {
    let theme = this.theme;
    let posts = channel_picker(
        this,
        "editor-posts",
        tr("ui.notification_settings_view.all_new_posts"),
        draft.posts,
        |d, v| d.posts = v,
        cx,
    );
    let mentions = channel_picker(
        this,
        "editor-mentions",
        tr("ui.notification_settings_view.channel_mentions"),
        draft.mentions,
        |d, v| d.mentions = v,
        cx,
    );
    let posts_off = draft.posts == ChannelChoice::Off;
    div()
        .flex()
        .flex_col()
        .gap_3()
        .child(posts)
        .child(switch_row(
            "editor-replies",
            tr("ui.notification_settings_view.include_all_replies"),
            None,
            !posts_off && draft.replies,
            posts_off,
            &theme,
            cx,
            |this, on, _| edit(this, |d| d.replies = on),
        ))
        .child(mentions)
        .child(widgets::divider(&theme))
        .child(switch_row(
            "editor-follow-all",
            tr("ui.notification_settings_view.follow_all_new_threads"),
            Some(tr(
                "ui.notification_settings_view.replies_to_every_post_are_notified",
            )),
            draft.follow_all,
            false,
            &theme,
            cx,
            |this, on, _| edit(this, |d| d.follow_all = on),
        ))
        .into_any_element()
}

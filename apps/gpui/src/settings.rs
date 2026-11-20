//! Settings pages, the account button and status menu, out of office, notification editor (owner: settings agent).
//! The macOS app's `SettingsScreen` (Screens.swift) and the sections it opens.
use crate::icons::{Icon, icon};
use crate::input::{InputEvent, TextInput};
use crate::oxide::{Oxide, tr, trf};
use crate::prefs::{Appearance, LanguagePref, RefreshPref};
use crate::settings_controls::{card, card_title, segmented};
use crate::settings_memory::MemoryReport;
use crate::settings_notifications::EditorDraft;
use crate::settings_updater::UpdateState;
use crate::widgets::{self, ButtonKind};
use crate::{
    settings_debug, settings_notifications, settings_out_of_office, settings_status,
    settings_updater,
};
use gpui::{AnyElement, Context, Entity, FontWeight, SharedString, Window, div, prelude::*, px};
use std::time::Instant;
use teams_core::{Action, MeAction, Nav, OutOfOffice, StatusDuration};

/// The settings sub-pages, opened from the settings home.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    Notifications,
    Refresh,
    Appearance,
    Language,
    AudioVideo,
    OutOfOffice,
    Debug,
}

pub struct Ui {
    /// The open sub-page; the home when `None`.
    pub page: Option<Page>,
    /// A page was opened from elsewhere and Settings is not shown yet: keep it.
    opening: bool,
    /// The menu a click outside just closed, so its trigger does not reopen it.
    pub just_closed: Option<(SharedString, Instant)>,
    /// A menu unfolded in place inside a popover or dialog.
    pub inline_open: Option<String>,

    pub status_duration: StatusDuration,
    pub note_sheet: bool,
    pub note_duration: StatusDuration,
    pub note_input: Entity<TextInput>,

    /// Settings › Notifications: the date of the "pause until" dialog, epoch ms.
    pub pause_until: Option<i64>,
    pub editor_draft: Option<EditorDraft>,

    /// The out of office form being edited, and the saved settings it started from.
    pub ooo_draft: Option<OutOfOffice>,
    pub ooo_base: Option<OutOfOffice>,
    pub ooo_internal: Entity<TextInput>,
    pub ooo_external: Entity<TextInput>,

    pub report: Option<MemoryReport>,
    pub measuring: bool,
    pub update: UpdateState,
}

impl Ui {
    pub fn new(_window: &mut Window, cx: &mut Context<Oxide>) -> Self {
        let note_input =
            cx.new(|cx| TextInput::new(cx, tr("ui.status_menu.status_message")).multiline(true));
        cx.subscribe(
            &note_input,
            |this: &mut Oxide, input, event: &InputEvent, cx| match event {
                InputEvent::Changed => {
                    // The service's limit, as the macOS app trims the editor.
                    let limit = this.me.as_ref().map(|m| m.note_limit).unwrap_or(2000) as usize;
                    let text = input.read(cx).text().to_owned();
                    if text.chars().count() > limit {
                        let cut: String = text.chars().take(limit).collect();
                        input.update(cx, |i, cx| i.set_text(cut, None, cx));
                    }
                    cx.notify();
                }
                InputEvent::Submit => settings_status::save_note(this, cx),
                InputEvent::Cancel => {
                    this.ui.settings.note_sheet = false;
                    cx.notify();
                }
                _ => {}
            },
        )
        .detach();
        let ooo_internal =
            cx.new(|cx| TextInput::new(cx, tr("ui.out_of_office.reply_inside")).multiline(false));
        let ooo_external = cx.new(|cx| {
            TextInput::new(cx, tr("ui.out_of_office.reply_outside_message")).multiline(false)
        });
        for (input, external) in [(&ooo_internal, false), (&ooo_external, true)] {
            cx.subscribe(
                input,
                move |this: &mut Oxide, input, event: &InputEvent, cx| {
                    if let InputEvent::Changed = event {
                        let text = input.read(cx).text().to_owned();
                        if let Some(draft) = this.ui.settings.ooo_draft.as_mut() {
                            if external {
                                draft.external_message = text;
                            } else {
                                draft.internal_message = text;
                            }
                        }
                        cx.notify();
                    }
                },
            )
            .detach();
        }
        Self {
            page: None,
            opening: false,
            just_closed: None,
            inline_open: None,
            status_duration: StatusDuration::Never,
            note_sheet: false,
            note_duration: StatusDuration::Never,
            note_input,
            pause_until: None,
            editor_draft: None,
            ooo_draft: None,
            ooo_base: None,
            ooo_internal,
            ooo_external,
            report: None,
            measuring: false,
            update: UpdateState::Idle,
        }
    }
}

/// Opens a settings page from anywhere (the status card, the call window's device button).
pub fn open(this: &mut Oxide, page: Page, cx: &mut Context<Oxide>) {
    let shown = this.shell.as_ref().is_some_and(|s| s.nav == Nav::Settings);
    this.ui.settings.opening = !shown;
    show_page(this, Some(page), cx);
    if !shown {
        this.send(Action::Navigate { nav: Nav::Settings });
    }
}

fn show_page(this: &mut Oxide, page: Option<Page>, cx: &mut Context<Oxide>) {
    this.ui.settings.page = page;
    this.ui.settings.inline_open = None;
    match page {
        Some(Page::OutOfOffice) => settings_out_of_office::entered(this, cx),
        Some(Page::Debug) => settings_debug::entered(this, cx),
        _ => {}
    }
    cx.notify();
}

/// Called every frame with the top bar: coming back to Settings starts from its home.
fn track_nav(this: &mut Oxide) {
    let in_settings = this.shell.as_ref().is_some_and(|s| s.nav == Nav::Settings);
    let ui = &mut this.ui.settings;
    if in_settings {
        ui.opening = false;
    } else if !ui.opening && ui.page.is_some() {
        ui.page = None;
        ui.report = None;
        ui.ooo_draft = None;
        ui.ooo_base = None;
    }
}

pub fn screen(this: &mut Oxide, window: &mut Window, cx: &mut Context<Oxide>) -> AnyElement {
    let body = match this.ui.settings.page {
        None => home(this, cx),
        Some(Page::Notifications) => settings_notifications::page(this, cx),
        Some(Page::Refresh) => {
            let body = refresh_section(this, cx);
            subpage(this, tr("ui.screens.message_refresh"), vec![body], cx)
        }
        Some(Page::Appearance) => {
            let body = appearance_section(this, cx);
            subpage(this, tr("ui.screens.appearance"), vec![body], cx)
        }
        Some(Page::Language) => {
            let sections = vec![language_section(this, cx), spelling_section(this, cx)];
            subpage(this, tr("ui.screens.language"), sections, cx)
        }
        Some(Page::OutOfOffice) => settings_out_of_office::page(this, window, cx),
        Some(Page::AudioVideo) => {
            let body = crate::call_devices::device_settings(this, window, cx);
            subpage(this, tr("ui.screens.audio_and_video"), vec![body], cx)
        }
        Some(Page::Debug) => {
            let body = settings_debug::section(this, cx);
            subpage(this, tr("ui.debug.title"), vec![body], cx)
        }
    };
    div()
        .id("settings-scroll")
        .size_full()
        .overflow_y_scroll()
        .child(
            div()
                .w_full()
                .flex()
                .justify_center()
                .p(px(32.))
                .child(div().w_full().max_w(px(760.)).child(body)),
        )
        .into_any_element()
}

/// A settings sub-page: back button and title, then its sections.
pub fn subpage(
    this: &mut Oxide,
    title: String,
    sections: Vec<AnyElement>,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    let theme = this.theme;
    div()
        .flex()
        .flex_col()
        .gap(px(22.))
        .child(
            div()
                .flex()
                .items_center()
                .gap_2()
                .child(
                    widgets::icon_button_sized(
                        "settings-back",
                        Icon::ChevronLeft,
                        34.,
                        16.,
                        false,
                        &theme,
                    )
                    .tooltip(widgets::tooltip(tr("ui.screens.back_to_settings"), theme))
                    .on_click(cx.listener(|this, _, _, cx| show_page(this, None, cx))),
                )
                .child(
                    div()
                        .text_size(px(24.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(title),
                ),
        )
        .children(sections)
        .into_any_element()
}

fn home(this: &mut Oxide, cx: &mut Context<Oxide>) -> AnyElement {
    let theme = this.theme;
    let Some(shell) = this.shell.clone() else {
        return div().into_any_element();
    };
    let account = widgets::card(&theme)
        .p(px(20.))
        .flex()
        .items_center()
        .gap_4()
        .child(this.avatar(
            &shell.me,
            &shell.display_name,
            48.,
            teams_core::AvatarShape::Round,
            None,
        ))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(5.))
                .child(
                    div()
                        .text_size(px(17.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(shell.display_name.clone()),
                )
                .when(!shell.account_email.is_empty(), |d| {
                    d.child(widgets::muted(shell.account_email.clone(), 12., &theme))
                })
                .child(
                    div()
                        .pt(px(2.))
                        .flex()
                        .items_center()
                        .gap_1()
                        .text_size(px(11.))
                        .text_color(theme.subtle)
                        .child(icon(Icon::Lock, px(11.), theme.subtle))
                        .child(tr("ui.screens.microsoft_sign_in_session_saved_in")),
                ),
        )
        .child(
            widgets::button(
                "settings-sign-out",
                tr("ui.screens.sign_out"),
                ButtonKind::Outline,
                &theme,
            )
            .on_click(cx.listener(|this, _, _, _| this.send(Action::SignOut))),
        );

    let notifications_summary = match &this.notifications {
        Some(v) if v.paused_until.is_some() => trf(
            "ui.notification_settings_view.paused_until",
            &[("until", v.paused_until.as_deref().unwrap_or_default())],
        ),
        Some(v) if v.permission == teams_core::NotificationPermission::Denied => {
            tr("ui.notification_settings_view.denied_by_macos")
        }
        _ => tr("ui.notification_settings_view.banners_sounds_meetings_email"),
    };
    let refresh_label = refresh_label(this.prefs.refresh);
    let refresh_summary = match &this.notifications {
        Some(v) if !v.realtime_live => trf(
            "ui.refresh_settings.notification_channel_disconnected_2",
            &[("label", &refresh_label)],
        ),
        _ => refresh_label,
    };
    let ooo_summary = if this.me.as_ref().is_some_and(|m| m.out_of_office) {
        tr("ui.status_menu.automatic_replies_on")
    } else {
        tr("ui.out_of_office.summary")
    };
    let memory = crate::settings_memory::size(crate::settings_memory::footprint());
    let links = [
        (
            Icon::Bell,
            tr("ui.notification_settings_view.notifications_and_activity"),
            notifications_summary,
            Page::Notifications,
        ),
        (
            Icon::Refresh,
            tr("ui.refresh_settings.message_refresh"),
            refresh_summary,
            Page::Refresh,
        ),
        (
            Icon::Contrast,
            tr("ui.screens.appearance"),
            appearance_label(this.prefs.appearance),
            Page::Appearance,
        ),
        (
            Icon::Globe,
            tr("ui.screens.language"),
            language_label(this.prefs.language),
            Page::Language,
        ),
        (
            Icon::Airplane,
            tr("ui.out_of_office.title"),
            ooo_summary,
            Page::OutOfOffice,
        ),
        (
            Icon::Video,
            tr("ui.screens.audio_and_video"),
            tr("ui.screens.microphone_speaker_camera_test_call"),
            Page::AudioVideo,
        ),
        (
            Icon::Chip,
            tr("ui.debug.title"),
            trf("ui.debug.summary", &[("size", &memory)]),
            Page::Debug,
        ),
    ];
    // Two columns: the window's minimum width always leaves room for both.
    let mut grid = div().flex().flex_col().gap_3();
    let mut row = Vec::new();
    for (ix, (glyph, title, summary, page)) in links.into_iter().enumerate() {
        row.push(link(ix, glyph, title, summary, page, &theme, cx));
        if row.len() == 2 || ix == 6 {
            let mut line = div().flex().gap_3();
            let filled = row.len();
            for cell in row.drain(..) {
                line = line.child(div().flex_1().min_w_0().flex().child(cell));
            }
            if filled == 1 {
                line = line.child(div().flex_1());
            }
            grid = grid.child(line);
        }
    }

    let version = trf(
        "ui.screens.version",
        &[("version", &teams_core::app_version())],
    );
    let foot = div()
        .w_full()
        .pt(px(14.))
        .flex()
        .flex_col()
        .items_center()
        .gap(px(6.))
        .child(crate::shell::logo(&theme))
        .child(
            div()
                .text_size(px(13.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.text_soft)
                .child("Oxide"),
        )
        .child(widgets::muted(version, 11., &theme))
        .children(settings_updater::row(this, cx));

    div()
        .flex()
        .flex_col()
        .gap(px(22.))
        .child(
            div()
                .text_size(px(24.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(tr("ui.screens.your_account")),
        )
        .child(account)
        .child(grid)
        .child(foot)
        .into_any_element()
}

/// A card of the settings home that opens a sub-page.
fn link(
    ix: usize,
    glyph: Icon,
    title: String,
    summary: String,
    page: Page,
    theme: &crate::theme::Theme,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    let hover = theme.input;
    widgets::card(theme)
        .id(SharedString::from(format!("settings-link-{ix}")))
        .flex_1()
        .p(px(18.))
        .flex()
        .items_center()
        .gap_3()
        .cursor_pointer()
        .hover(move |s| s.bg(hover))
        .child(
            div()
                .w(px(22.))
                .flex()
                .justify_center()
                .child(icon(glyph, px(17.), theme.accent)),
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
                        .text_size(px(14.))
                        .font_weight(FontWeight::SEMIBOLD)
                        .child(title),
                )
                .child(widgets::muted(summary, 12., theme)),
        )
        .child(icon(Icon::ChevronRight, px(13.), theme.subtle))
        .on_click(cx.listener(move |this, _, _, cx| show_page(this, Some(page), cx)))
        .into_any_element()
}

fn appearance_label(appearance: Appearance) -> String {
    tr(match appearance {
        Appearance::System => "ui.theme.system",
        Appearance::Light => "ui.theme.light",
        Appearance::Dark => "ui.theme.dark",
    })
}

fn language_label(language: LanguagePref) -> String {
    match language {
        LanguagePref::System => tr("ui.localization.system"),
        LanguagePref::French => "Français".to_owned(),
        LanguagePref::English => "English".to_owned(),
    }
}

fn refresh_label(refresh: RefreshPref) -> String {
    tr(match refresh {
        RefreshPref::Push => "ui.refresh_settings.real_time",
        RefreshPref::Poll => "ui.refresh_settings.constant_refresh",
    })
}

/// Settings › Appearance.
fn appearance_section(this: &mut Oxide, cx: &mut Context<Oxide>) -> AnyElement {
    let theme = this.theme;
    let options = [Appearance::System, Appearance::Light, Appearance::Dark]
        .into_iter()
        .map(|a| (a, appearance_label(a)))
        .collect();
    card(&theme)
        .gap_2()
        .child(card_title(tr("ui.screens.theme")))
        .child(div().flex().child(segmented(
            "appearance",
            options,
            this.prefs.appearance,
            &theme,
            cx,
            |this, value, _, cx| this.set_appearance(value, cx),
        )))
        .into_any_element()
}

/// Settings › Language.
fn language_section(this: &mut Oxide, cx: &mut Context<Oxide>) -> AnyElement {
    let theme = this.theme;
    let options = [
        LanguagePref::System,
        LanguagePref::French,
        LanguagePref::English,
    ]
    .into_iter()
    .map(|l| (l, language_label(l)))
    .collect();
    card(&theme)
        .gap_2()
        .child(card_title(tr("ui.localization.app_language")))
        .child(div().flex().child(segmented(
            "language",
            options,
            this.prefs.language,
            &theme,
            cx,
            |this, value, _, cx| this.set_language(value, cx),
        )))
        .child(widgets::muted(
            tr("ui.localization.system_menus_follow_next_launch"),
            12.,
            &theme,
        ))
        .into_any_element()
}

/// Settings › Language: spelling correction in the composer (the composer reads `prefs.spelling`).
fn spelling_section(this: &mut Oxide, cx: &mut Context<Oxide>) -> AnyElement {
    let theme = this.theme;
    card(&theme)
        .gap_2()
        .child(
            div()
                .flex()
                .items_center()
                .gap_3()
                .child(
                    card_title(tr("ui.spelling_settings.correct_spelling_automatically")).flex_1(),
                )
                .child(
                    widgets::toggle("spelling", this.prefs.spelling, &theme).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.prefs.spelling = !this.prefs.spelling;
                            this.prefs.save();
                            cx.notify();
                        },
                    )),
                ),
        )
        .child(widgets::muted(
            tr("ui.spelling_settings.correct_spelling_automatically_detail"),
            12.,
            &theme,
        ))
        .into_any_element()
}

/// Settings › Message refresh: push only, or polling all the time.
fn refresh_section(this: &mut Oxide, cx: &mut Context<Oxide>) -> AnyElement {
    let theme = this.theme;
    let options = [RefreshPref::Push, RefreshPref::Poll]
        .into_iter()
        .map(|r| (r, refresh_label(r)))
        .collect();
    let detail = tr(match this.prefs.refresh {
        RefreshPref::Push => "ui.refresh_settings.push_detail",
        RefreshPref::Poll => "ui.refresh_settings.polling_detail",
    });
    card(&theme)
        .gap_2()
        .child(card_title(tr("ui.refresh_settings.mode")))
        .child(div().flex().child(segmented(
            "refresh",
            options,
            this.prefs.refresh,
            &theme,
            cx,
            |this, value, _, _| this.set_refresh(value),
        )))
        .child(widgets::muted(detail, 12., &theme))
        .when_some(
            this.notifications.as_ref().map(|v| v.realtime_live),
            |d, live| {
                let color = if live { theme.muted } else { theme.danger };
                d.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1p5()
                        .text_size(px(12.))
                        .text_color(color)
                        .child(icon(
                            if live {
                                Icon::Antenna
                            } else {
                                Icon::AntennaOff
                            },
                            px(13.),
                            color,
                        ))
                        .child(tr(if live {
                            "ui.refresh_settings.notification_channel_connected"
                        } else {
                            "ui.refresh_settings.notification_channel_disconnected"
                        })),
                )
            },
        )
        .into_any_element()
}

/// The top bar's account button (photo and availability), opening the status card.
pub fn account_button(
    this: &mut Oxide,
    window: &mut Window,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    track_nav(this);
    settings_status::account_button(this, window, cx)
}

/// Dialogs and floating panels of this module, drawn over the window.
pub fn overlay(
    this: &mut Oxide,
    window: &mut Window,
    cx: &mut Context<Oxide>,
) -> Option<AnyElement> {
    if let Some(sheet) = settings_status::note_sheet(this, window, cx) {
        return Some(sheet);
    }
    if let Some(sheet) = settings_notifications::editor_sheet(this, cx) {
        return Some(sheet);
    }
    if let Some(sheet) = settings_notifications::pause_sheet(this, cx) {
        return Some(sheet);
    }
    settings_updater::sheet(this, cx)
}

/// A step of `OXIDE_SCRIPT` this module understands (see `screens::run_script`); false if not its own.
/// `settings:<home|notifications|refresh|appearance|language|audio|out-of-office|debug>` opens a page;
/// `settings:account` opens the status card, `settings:note` the status message dialog,
/// `settings:pause` the "pause until" dialog, `settings:edit-chat:<n>` the n-th chat's notification
/// dialog, `settings:menu:<key>` a menu picker of the open page, `settings:status:<n>` picks the n-th status.
pub fn debug_step(
    this: &mut Oxide,
    step: &str,
    window: &mut gpui::Window,
    cx: &mut Context<Oxide>,
) -> bool {
    let Some(arg) = step.strip_prefix("settings:") else {
        return false;
    };
    let page = match arg {
        "home" => Some(None),
        "notifications" => Some(Some(Page::Notifications)),
        "refresh" => Some(Some(Page::Refresh)),
        "appearance" => Some(Some(Page::Appearance)),
        "language" => Some(Some(Page::Language)),
        "audio" => Some(Some(Page::AudioVideo)),
        "out-of-office" => Some(Some(Page::OutOfOffice)),
        "debug" => Some(Some(Page::Debug)),
        _ => None,
    };
    if let Some(page) = page {
        match page {
            Some(page) => open(this, page, cx),
            None => show_page(this, None, cx),
        }
        return true;
    }
    match arg.split_once(':').unwrap_or((arg, "")) {
        ("account", _) => settings_status::open_card(this, cx),
        ("note", _) => settings_status::open_note(this, window, cx),
        ("pause", _) => settings_notifications::open_pause(this),
        ("edit-chat", n) => {
            let Some(chat) = n
                .parse::<usize>()
                .ok()
                .and_then(|n| this.sidebar.as_ref()?.chats.get(n).cloned())
            else {
                return false;
            };
            this.send(Action::Notifications {
                action: teams_core::NotificationsAction::EditChat {
                    conversation_id: chat.id,
                },
            });
        }
        ("menu", key) => this.popup = Some(key.to_owned().into()),
        ("inline", key) => this.ui.settings.inline_open = Some(key.to_owned()),
        ("status", n) => {
            let Some(status) = n
                .parse::<usize>()
                .ok()
                .and_then(|n| settings_status::STATUSES.get(n).copied())
            else {
                return false;
            };
            let duration = this.ui.settings.status_duration;
            this.send(Action::Me {
                action: MeAction::SetStatus { status, duration },
            });
        }
        ("ooo-on", _) => {
            if let Some(draft) = this.ui.settings.ooo_draft.as_mut() {
                draft.enabled = true;
                draft.reply_outside = true;
                draft.scheduled = true;
            }
        }
        _ => return false,
    }
    cx.notify();
    true
}

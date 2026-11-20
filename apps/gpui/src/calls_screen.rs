//! The Calls screen: history, voicemail, dial (owner: settings agent). Calls/CallsScreen.swift and the
//! experimental dial panel it shows above the history (Calls/CallPanel.swift).
use crate::icons::{Icon, icon};
use crate::oxide::{Oxide, tr, trf};
use crate::settings_controls::segmented;
use crate::settings_time::{date_time_of, day_title, duration, parse_iso, time_of};
use crate::theme::Theme;
use crate::widgets;
use gpui::{
    AnyElement, Context, Corner, FontWeight, ListAlignment, ListState, MouseButton, Pixels,
    Point, SharedString, Window, anchored, deferred, div, list, prelude::*, px,
};
use teams_core::{
    Action, CallAction, CallDirectionView, CallLogView, CallOutcome,
    CallsAction, VoicemailView,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tab {
    History,
    Voicemail,
}

/// A line of the history list: a day's header, a call, or "Show older calls".
#[derive(Clone, Debug, PartialEq, Eq)]
enum Row {
    Day(String),
    Log(CallLogView),
    More,
}

/// The right-click menu of a call or a voicemail.
#[derive(Clone, Debug)]
enum Menu {
    Log(CallLogView),
    Voicemail(VoicemailView),
}

pub struct Ui {
    pub tab: Tab,
    rows: Vec<Row>,
    list: ListState,
    menu: Option<(Menu, Point<Pixels>)>,
    /// The voicemail asked for, until its audio is handed to the player.
    requested: Option<String>,
}

impl Ui {
    pub fn new(_window: &mut Window, _cx: &mut Context<Oxide>) -> Self {
        Self {
            tab: Tab::History,
            rows: Vec::new(),
            list: ListState::new(0, ListAlignment::Top, px(600.)),
            menu: None,
            requested: None,
        }
    }
}

fn send(this: &Oxide, action: CallsAction) {
    this.send(Action::Calls { action });
}

fn call(this: &Oxide, action: CallAction) {
    this.send(Action::Call { action });
}

/// Days newest first, as the history comes; the "more" line last.
fn rows_of(history: &[CallLogView], has_more: bool, now: i64) -> Vec<Row> {
    let mut rows = Vec::with_capacity(history.len() + 8);
    let mut last: Option<String> = None;
    for log in history {
        let title = day_title(parse_iso(&log.started_at), now);
        if last.as_ref() != Some(&title) {
            rows.push(Row::Day(title.clone()));
            last = Some(title);
        }
        rows.push(Row::Log(log.clone()));
    }
    if has_more && !history.is_empty() {
        rows.push(Row::More);
    }
    rows
}

/// Keeps the list's measured rows and scroll position: only the changed tail is measured again.
fn update_rows(ui: &mut Ui, rows: Vec<Row>) {
    if ui.rows == rows {
        return;
    }
    let same = ui
        .rows
        .iter()
        .zip(&rows)
        .take_while(|(a, b)| a == b)
        .count();
    if ui.rows.is_empty() {
        ui.list.reset(rows.len());
    } else {
        ui.list.splice(same..ui.rows.len(), rows.len() - same);
    }
    ui.rows = rows;
}

/// Follows the voicemail player: the audio for the asked voicemail arrived, or the message ended.
fn track_playback(this: &mut Oxide, cx: &mut Context<Oxide>) {
    let ready = this.calls.as_ref().and_then(|c| c.audio_ready.clone());
    if let (Some(ready), Some(requested)) = (ready, this.ui.calls.requested.clone())
        && ready == requested
    {
        // The platform player now has it: its state drives the play / stop button.
        let _ = ready;
        this.ui.calls.requested = None;
        cx.notify();
    }
}

pub fn screen(this: &mut Oxide, window: &mut Window, cx: &mut Context<Oxide>) -> AnyElement {
    track_playback(this, cx);
    let theme = this.theme;
    let model = this.calls.clone().unwrap_or_default();
    let unread = model.voicemails.iter().filter(|v| !v.read).count();
    let voicemail_title = if unread > 0 {
        format!("{} ({unread})", tr("ui.calls_screen.voicemail"))
    } else {
        tr("ui.calls_screen.voicemail")
    };
    let tabs = vec![
        (Tab::History, tr("ui.calls_screen.history")),
        (Tab::Voicemail, voicemail_title),
    ];
    let header = div()
        .flex()
        .items_center()
        .gap(px(14.))
        .px(px(24.))
        .py_4()
        .child(
            div()
                .text_size(px(22.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(tr("ui.calls_screen.calls")),
        )
        .child(segmented(
            "calls-tabs",
            tabs,
            this.ui.calls.tab,
            &theme,
            cx,
            |this, tab, _, _| {
                this.ui.calls.tab = tab;
                this.ui.calls.menu = None;
            },
        ))
        .child(div().flex_1())
        .when(model.loading_history || model.loading_voicemails, |d| {
            d.child(widgets::spinner("calls-loading", 14., theme.muted))
        })
        .child(
            widgets::icon_button_sized("calls-refresh", Icon::Refresh, 34., 16., false, &theme)
                .tooltip(widgets::tooltip(tr("ui.calls_screen.refresh"), theme))
                .on_click(cx.listener(|this, _, _, _| send(this, CallsAction::Refresh))),
        );
    let body = match this.ui.calls.tab {
        Tab::History => history(this, &model, window, cx),
        Tab::Voicemail => voicemails(this, &model, cx),
    };
    div()
        .size_full()
        .flex()
        .flex_col()
        .bg(theme.surface)
        .child(header)
        .child(widgets::divider(&theme))
        .child(body)
        .children(context_menu(this, cx))
        .into_any_element()
}

/// A screen's load error across its width, with a button to try again.
fn banner(text: String, theme: &Theme, cx: &mut Context<Oxide>) -> AnyElement {
    div()
        .flex()
        .items_center()
        .gap_2()
        .p_3()
        .bg(theme.error_background)
        .text_size(px(12.))
        .text_color(theme.error_text)
        .child(icon(Icon::Alert, px(13.), theme.error_text))
        .child(div().flex_1().child(text))
        .child(
            div()
                .id("calls-retry")
                .cursor_pointer()
                .font_weight(FontWeight::MEDIUM)
                .child(tr("ui.calls_screen.try_again"))
                .on_click(cx.listener(|this, _, _, _| send(this, CallsAction::Refresh))),
        )
        .into_any_element()
}

fn empty(text: String, glyph: Icon, theme: &Theme) -> AnyElement {
    div()
        .flex_1()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(10.))
        .child(icon(glyph, px(30.), theme.subtle))
        .child(widgets::muted(text, 14., theme))
        .into_any_element()
}

fn history(
    this: &mut Oxide,
    model: &teams_core::CallsView,
    window: &mut Window,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    let theme = this.theme;
    // The calls agent's port of CallPanel (call_panel.rs) is the one with video.
    let panel = crate::call_panel::panel(this, window, cx);
    let rows = rows_of(&model.history, model.has_more, crate::settings_time::now());
    update_rows(&mut this.ui.calls, rows);
    let list_or_empty = if model.history.is_empty() {
        let text = tr(if model.loading_history {
            "ui.calls_screen.loading_history"
        } else {
            "ui.calls_screen.no_recent_calls"
        });
        empty(text, Icon::Phone, &theme)
    } else {
        let entity = cx.entity();
        list(this.ui.calls.list.clone(), move |ix, window, cx| {
            entity.update(cx, |this, cx| history_row(this, ix, window, cx))
        })
        .flex_1()
        .into_any_element()
    };
    div()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .child(div().p_4().child(panel))
        .when_some(model.error.clone(), |d, error| {
            d.child(banner(error, &theme, cx))
        })
        .child(list_or_empty)
        .into_any_element()
}

fn history_row(
    this: &mut Oxide,
    ix: usize,
    _window: &mut Window,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    let theme = this.theme;
    let Some(row) = this.ui.calls.rows.get(ix).cloned() else {
        return div().into_any_element();
    };
    match row {
        Row::Day(title) => div()
            .w_full()
            .px(px(24.))
            .pt_3()
            .pb_1()
            .text_size(px(12.))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(theme.muted)
            .child(title)
            .into_any_element(),
        Row::More => {
            let loading = this.calls.as_ref().is_some_and(|c| c.loading_history);
            div()
                .px(px(24.))
                .py_3()
                .child(
                    div()
                        .id("calls-more")
                        .text_size(px(13.))
                        .text_color(theme.accent_text)
                        .when(loading, |d| d.opacity(0.45))
                        .when(!loading, |d| d.cursor_pointer())
                        .child(tr("ui.calls_screen.show_older_calls"))
                        .on_click(cx.listener(move |this, _, _, _| {
                            if !loading {
                                send(this, CallsAction::More)
                            }
                        })),
                )
                .into_any_element()
        }
        Row::Log(log) => log_row(this, log, cx),
    }
}

/// "Missed call · 3m 05s"-style summary, as `CallsFormat.summary`.
fn summary(log: &CallLogView) -> String {
    let kind = match (log.outcome, log.direction) {
        (CallOutcome::Missed, _) => "missed",
        (CallOutcome::Declined, _) => "declined",
        (_, CallDirectionView::Incoming) => "incoming",
        (_, CallDirectionView::Outgoing) => "outgoing",
        (_, CallDirectionView::Unknown) => "other",
    };
    let label = tr(&format!(
        "ui.calls_screen.summary.{kind}{}",
        if log.group { "_group" } else { "" }
    ));
    match log.duration_secs {
        Some(secs) if !matches!(log.outcome, CallOutcome::Missed | CallOutcome::Declined) => trf(
            "ui.calls_screen.summary.with_duration",
            &[("label", &label), ("duration", &duration(secs))],
        ),
        _ => label,
    }
}

fn symbol(log: &CallLogView) -> Icon {
    if matches!(log.outcome, CallOutcome::Missed | CallOutcome::Declined) {
        return Icon::PhoneOff;
    }
    match log.direction {
        CallDirectionView::Incoming => Icon::PhoneIncoming,
        CallDirectionView::Outgoing => Icon::PhoneOutgoing,
        CallDirectionView::Unknown => Icon::Phone,
    }
}

fn log_row(this: &mut Oxide, log: CallLogView, cx: &mut Context<Oxide>) -> AnyElement {
    let theme = this.theme;
    let can_call = log.can_call_back && !this.in_call();
    let avatar_id = if log.peer.is_empty() {
        log.id.clone()
    } else {
        log.peer.clone()
    };
    let avatar = this.avatar(
        &avatar_id,
        &log.name,
        36.,
        teams_core::AvatarShape::Round,
        None,
    );
    let missed = log.outcome == CallOutcome::Missed;
    let id = log.id.clone();
    let call_back =
        |suffix: &str, glyph: Icon, help: String, video: bool, cx: &mut Context<Oxide>| {
            let id = id.clone();
            widgets::icon_button_sized(
                SharedString::from(format!("call-back-{suffix}-{id}")),
                glyph,
                30.,
                14.,
                false,
                &theme,
            )
            .tooltip(widgets::tooltip(help, theme))
            .on_click(cx.listener(move |this, _, _, _| {
                send(
                    this,
                    CallsAction::CallBack {
                        id: id.clone(),
                        video,
                    },
                )
            }))
        };
    let menu_log = log.clone();
    div()
        .id(SharedString::from(format!("call-log-{}", log.id)))
        .w_full()
        .px(px(24.))
        .py(px(6.))
        .flex()
        .items_center()
        .gap_3()
        .child(avatar)
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
                        .text_color(if missed { theme.danger } else { theme.text })
                        .child(log.name.clone()),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(5.))
                        .text_size(px(12.))
                        .text_color(theme.muted)
                        .child(icon(symbol(&log), px(12.), theme.muted))
                        .child(summary(&log)),
                ),
        )
        .child(
            div()
                .text_size(px(12.))
                .text_color(theme.subtle)
                .child(time_of(&log.started_at)),
        )
        .when(can_call, |d| {
            d.child(call_back(
                "audio",
                Icon::Phone,
                tr("ui.calls_screen.call_back"),
                false,
                cx,
            ))
            .child(call_back(
                "video",
                Icon::Video,
                tr("ui.calls_screen.video_call_back"),
                true,
                cx,
            ))
        })
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                this.ui.calls.menu = Some((Menu::Log(menu_log.clone()), event.position));
                cx.notify();
            }),
        )
        .into_any_element()
}

fn voicemails(
    this: &mut Oxide,
    model: &teams_core::CallsView,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    let theme = this.theme;
    let banner = model
        .voicemail_error
        .clone()
        .map(|error| banner(error, &theme, cx));
    let body = if model.voicemails.is_empty() {
        let text = tr(if model.loading_voicemails {
            "ui.calls_screen.loading_voicemail"
        } else {
            "ui.calls_screen.no_voicemail"
        });
        empty(text, Icon::Voicemail, &theme)
    } else {
        // A mailbox holds a few dozen messages at most: a plain scrolled column.
        let mut column = div()
            .id("voicemails")
            .flex_1()
            .min_h_0()
            .overflow_y_scroll()
            .py_2()
            .flex()
            .flex_col();
        for voicemail in model.voicemails.clone() {
            column = column.child(voicemail_row(
                this,
                voicemail,
                model.audio_loading.as_deref(),
                cx,
            ));
        }
        column.into_any_element()
    };
    div()
        .flex_1()
        .min_h_0()
        .flex()
        .flex_col()
        .children(banner)
        .child(body)
        .into_any_element()
}

fn voicemail_row(
    this: &mut Oxide,
    voicemail: VoicemailView,
    loading: Option<&str>,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    let theme = this.theme;
    let playing = crate::platform::voicemail_playing().as_deref() == Some(voicemail.id.as_str());
    let loading = loading == Some(voicemail.id.as_str());
    let id = voicemail.id.clone();
    let play = div()
        .id(SharedString::from(format!("voicemail-play-{id}")))
        .size(px(34.))
        .flex_none()
        .rounded_full()
        .bg(theme.accent_soft)
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .child(if loading {
            widgets::spinner(
                SharedString::from(format!("voicemail-spin-{id}")),
                14.,
                theme.accent_text,
            )
        } else {
            icon(
                if playing { Icon::Stop } else { Icon::Play },
                px(14.),
                theme.accent_text,
            )
            .into_any_element()
        })
        .tooltip(widgets::tooltip(
            tr(if playing {
                "ui.calls_screen.stop"
            } else {
                "ui.calls_screen.play"
            }),
            theme,
        ))
        .on_click(cx.listener(move |this, _, _, cx| {
            if crate::platform::voicemail_playing().as_deref() == Some(id.as_str()) {
                crate::platform::stop_voicemail();
            } else {
                this.ui.calls.requested = Some(id.clone());
                send(this, CallsAction::PlayVoicemail { id: id.clone() });
            }
            cx.notify();
        }));
    let mut details = div().flex().items_center().gap_2();
    if !voicemail.address.is_empty() && voicemail.address != voicemail.sender {
        details = details.child(widgets::muted(voicemail.address.clone(), 12., &theme));
    }
    if let Some(secs) = voicemail.duration_secs {
        details = details.child(widgets::muted(duration(secs as i64), 12., &theme));
    }
    let menu_voicemail = voicemail.clone();
    div()
        .id(SharedString::from(format!("voicemail-{}", voicemail.id)))
        .px(px(24.))
        .py(px(10.))
        .flex()
        .items_start()
        .gap_3()
        .child(play)
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap_1()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .when(!voicemail.read, |d| {
                            d.child(div().size(px(7.)).rounded_full().bg(theme.accent))
                        })
                        .child(
                            div()
                                .text_size(px(14.))
                                .when(!voicemail.read, |d| d.font_weight(FontWeight::SEMIBOLD))
                                .child(voicemail.sender.clone()),
                        )
                        .child(div().flex_1())
                        .child(
                            div()
                                .text_size(px(12.))
                                .text_color(theme.subtle)
                                .child(date_time_of(&voicemail.received)),
                        ),
                )
                .child(details)
                .when_some(voicemail.transcription.clone(), |d, text| {
                    d.child(
                        div()
                            .text_size(px(13.))
                            .text_color(theme.text_soft)
                            .child(text),
                    )
                }),
        )
        .on_mouse_down(
            MouseButton::Right,
            cx.listener(move |this, event: &gpui::MouseDownEvent, _, cx| {
                this.ui.calls.menu =
                    Some((Menu::Voicemail(menu_voicemail.clone()), event.position));
                cx.notify();
            }),
        )
        .into_any_element()
}

/// The right-click menu, where the click was.
fn context_menu(this: &mut Oxide, cx: &mut Context<Oxide>) -> Option<AnyElement> {
    let (menu, position) = this.ui.calls.menu.clone()?;
    let theme = this.theme;
    let close = |this: &mut Oxide, cx: &mut Context<Oxide>| {
        this.ui.calls.menu = None;
        cx.notify();
    };
    let mut items = div().flex().flex_col();
    let danger = |item: gpui::Stateful<gpui::Div>| item.text_color(theme.danger);
    match menu {
        Menu::Log(log) => {
            if log.can_call_back && !this.in_call() {
                for (ix, (label, video)) in [
                    (tr("ui.calls_screen.call_back"), false),
                    (tr("ui.calls_screen.video_call_back"), true),
                ]
                .into_iter()
                .enumerate()
                {
                    let id = log.id.clone();
                    items = items.child(
                        widgets::menu_item(
                            SharedString::from(format!("log-menu-{ix}")),
                            None,
                            label,
                            &theme,
                        )
                        .on_click(cx.listener(move |this, _, _, cx| {
                            send(
                                this,
                                CallsAction::CallBack {
                                    id: id.clone(),
                                    video,
                                },
                            );
                            close(this, cx);
                        })),
                    );
                }
                items = items.child(div().my_1().child(widgets::divider(&theme)));
            }
            let id = log.id.clone();
            items = items.child(
                danger(widgets::menu_item(
                    "log-menu-delete",
                    Some(Icon::Trash),
                    tr("ui.calls_screen.delete_from_history"),
                    &theme,
                ))
                .on_click(cx.listener(move |this, _, _, cx| {
                    send(this, CallsAction::DeleteLog { id: id.clone() });
                    close(this, cx);
                })),
            );
        }
        Menu::Voicemail(voicemail) => {
            let id = voicemail.id.clone();
            let read = voicemail.read;
            let label = tr(if read {
                "ui.calls_screen.mark_as_unread"
            } else {
                "ui.calls_screen.mark_as_read"
            });
            items = items.child(
                widgets::menu_item("voicemail-menu-read", None, label, &theme).on_click(
                    cx.listener(move |this, _, _, cx| {
                        send(
                            this,
                            CallsAction::MarkVoicemail {
                                id: id.clone(),
                                read: !read,
                            },
                        );
                        close(this, cx);
                    }),
                ),
            );
            let id = voicemail.id.clone();
            items = items.child(
                danger(widgets::menu_item(
                    "voicemail-menu-delete",
                    Some(Icon::Trash),
                    tr("ui.calls_screen.delete"),
                    &theme,
                ))
                .on_click(cx.listener(move |this, _, _, cx| {
                    send(this, CallsAction::DeleteVoicemail { id: id.clone() });
                    close(this, cx);
                })),
            );
        }
    }
    let entity = cx.entity().downgrade();
    Some(
        deferred(
            anchored()
                .position(position)
                .anchor(Corner::TopLeft)
                .snap_to_window_with_margin(px(8.))
                .child(
                    div()
                        .occlude()
                        .rounded(px(10.))
                        .border_1()
                        .border_color(theme.border)
                        .bg(theme.surface)
                        .shadow_lg()
                        .py_1()
                        .on_mouse_down_out(move |_, _, cx| {
                            let _ = entity.update(cx, |this, cx| {
                                this.ui.calls.menu = None;
                                cx.notify();
                            });
                        })
                        .child(items),
                ),
        )
        .with_priority(1)
        .into_any_element(),
    )
}

pub fn overlay(
    this: &mut Oxide,
    window: &mut Window,
    cx: &mut Context<Oxide>,
) -> Option<AnyElement> {
    let _ = (this, window, cx);
    None
}

/// A step of `OXIDE_SCRIPT` this module understands (see `screens::run_script`); false if not its own.
/// `calls:history`, `calls:voicemail` pick the tab; `calls:menu:<n>` opens the n-th call's (or, on the
/// voicemail tab, voicemail's) right-click menu; `calls:play:<n>` plays the n-th voicemail;
/// `calls:dial:<text>` types in the dial field.
pub fn debug_step(
    this: &mut Oxide,
    step: &str,
    window: &mut gpui::Window,
    cx: &mut Context<Oxide>,
) -> bool {
    let Some(arg) = step.strip_prefix("calls:") else {
        return false;
    };
    let (verb, rest) = arg.split_once(':').unwrap_or((arg, ""));
    match verb {
        "history" => this.ui.calls.tab = Tab::History,
        "voicemail" => this.ui.calls.tab = Tab::Voicemail,
        "menu" => {
            let n: usize = rest.parse().unwrap_or(0);
            let Some(model) = this.calls.clone() else {
                return false;
            };
            let menu = match this.ui.calls.tab {
                Tab::History => model.history.get(n).cloned().map(Menu::Log),
                Tab::Voicemail => model.voicemails.get(n).cloned().map(Menu::Voicemail),
            };
            let Some(menu) = menu else { return false };
            let y = 260. + 52. * n as f32;
            this.ui.calls.menu = Some((menu, gpui::point(px(520.), px(y))));
        }
        "play" => {
            let Some(id) = this
                .calls
                .as_ref()
                .and_then(|c| c.voicemails.get(rest.parse::<usize>().unwrap_or(0)))
                .map(|v| v.id.clone())
            else {
                return false;
            };
            this.ui.calls.requested = Some(id.clone());
            send(this, CallsAction::PlayVoicemail { id });
        }
        "dial" => {
            let text = rest.to_owned();
            this.ui
                .call
                .panel_target
                .update(cx, |i, cx| i.set_text(text, None, cx));
            let _ = window;
        }
        _ => return false,
    }
    cx.notify();
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn log(id: &str, at: &str) -> CallLogView {
        CallLogView {
            id: id.to_owned(),
            name: "Alice".to_owned(),
            peer: String::new(),
            direction: CallDirectionView::Incoming,
            outcome: CallOutcome::Answered,
            started_at: at.to_owned(),
            duration_secs: Some(65),
            group: false,
            can_call_back: true,
        }
    }

    /// Local wall time as the services write it (UTC), so the test holds in any time zone.
    fn utc_iso(epoch: i64) -> String {
        let (days, secs) = (epoch.div_euclid(86_400), epoch.rem_euclid(86_400));
        let z = days + 719_468;
        let era = z.div_euclid(146_097);
        let doe = z - era * 146_097;
        let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
        let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
        let mp = (5 * doy + 2) / 153;
        let day = doy - (153 * mp + 2) / 5 + 1;
        let month = if mp < 10 { mp + 3 } else { mp - 9 };
        let year = yoe + era * 400 + i64::from(month <= 2);
        format!(
            "{year:04}-{month:02}-{day:02}T{:02}:{:02}:00Z",
            secs / 3600,
            secs % 3600 / 60
        )
    }

    #[test]
    fn groups_by_day() {
        let now = crate::settings_time::from_local(2026, 11, 20, 15, 0);
        let iso = |d: i32, h: i32| utc_iso(crate::settings_time::from_local(2026, 11, d, h, 0));
        let history = vec![
            log("a", &iso(20, 10)),
            log("b", &iso(20, 9)),
            log("c", &iso(19, 9)),
            log("d", ""),
        ];
        let rows = rows_of(&history, true, now);
        let days: Vec<&Row> = rows.iter().filter(|r| matches!(r, Row::Day(_))).collect();
        assert_eq!(days.len(), 3);
        assert_eq!(rows.len(), 3 + 4 + 1);
        assert_eq!(rows.last(), Some(&Row::More));
        assert_eq!(rows[0], Row::Day(tr("ui.calls_screen.today")));
        assert_eq!(rows[3], Row::Day(tr("ui.calls_screen.yesterday")));
    }

    #[test]
    fn summaries() {
        let mut l = log("a", "");
        assert!(summary(&l).contains(&duration(65)));
        l.outcome = CallOutcome::Missed;
        assert!(!summary(&l).contains(&duration(65)));
    }
}

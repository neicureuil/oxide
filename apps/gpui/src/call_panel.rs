//! Experimental native calling on the Calls screen (`CallPanel.swift`): place a 1:1 call or join a meeting, then
//! control it. The Calls screen draws it with `call_panel::panel`.
use crate::icons::{Icon, icon};
use crate::input::{InputEvent, TextInput};
use crate::media_video;
use crate::oxide::{Oxide, tr, trf};
use crate::widgets::{self, ButtonKind};
use gpui::{AnyElement, Context, Entity, FontWeight, Window, div, prelude::*, px};
use teams_core::{Action, ActiveCallView, AvatarShape, CallAction, CallPhaseView};

/// The target field, made with the call module's state.
pub fn target_field(cx: &mut Context<Oxide>) -> Entity<TextInput> {
    let input = cx.new(|cx| TextInput::new(cx, tr("ui.call_panel.8_orgid_or_teams_meeting_link")));
    cx.subscribe(&input, |this, input, event: &InputEvent, cx| {
        if *event == InputEvent::Submit {
            let target = input.read(cx).text().trim().to_owned();
            start_target(this, target, false);
        }
    })
    .detach();
    input
}

fn send(this: &Oxide, action: CallAction) {
    this.send(Action::Call { action });
}

fn target(this: &Oxide, cx: &gpui::App) -> String {
    this.ui.call.panel_target.read(cx).text().trim().to_owned()
}

fn is_meeting(target: &str) -> bool {
    target.contains("teams.") && target.starts_with("https://")
}

fn start_target(this: &Oxide, target: String, video: bool) {
    if target.is_empty() {
        return;
    }
    if is_meeting(&target) {
        send(this, CallAction::JoinMeeting { url: target, video });
    } else {
        send(this, CallAction::Start { mri: target, video });
    }
}

pub fn panel(this: &mut Oxide, window: &mut Window, cx: &mut Context<Oxide>) -> AnyElement {
    let _ = window;
    let theme = this.theme;
    let status = this
        .calls
        .as_ref()
        .map(|c| c.realtime.clone())
        .filter(|s| !s.is_empty());
    let part = this.call.clone().unwrap_or_default();
    let body = match &part.active {
        Some(call) => active_box(this, call, cx),
        None => {
            let text = target(this, cx);
            let meeting = is_meeting(&text);
            let empty = text.is_empty();
            let field = widgets::text_field(&this.ui.call.panel_target, &theme, cx).flex_1();
            let action = |id: &'static str, label: String, kind: ButtonKind, video: bool| {
                widgets::disabled(widgets::button(id, label, kind, &theme), empty).when(
                    !empty,
                    |b| {
                        b.on_click(cx.listener(move |this, _, _, cx| {
                            let target = target(this, cx);
                            start_target(this, target, video);
                        }))
                    },
                )
            };
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(field)
                .map(|d| {
                    if meeting {
                        d.child(action(
                            "panel-join",
                            tr("ui.call_panel.join"),
                            ButtonKind::Primary,
                            false,
                        ))
                        .child(action(
                            "panel-join-video",
                            tr("ui.call_panel.with_video"),
                            ButtonKind::Outline,
                            true,
                        ))
                    } else {
                        d.child(action(
                            "panel-audio",
                            tr("ui.call_panel.audio_call"),
                            ButtonKind::Primary,
                            false,
                        ))
                        .child(action(
                            "panel-video",
                            tr("ui.call_panel.video"),
                            ButtonKind::Outline,
                            true,
                        ))
                    }
                })
                .into_any_element()
        }
    };
    widgets::card(&theme)
        .p(px(16.))
        .flex()
        .flex_col()
        .gap(px(12.))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(6.))
                .text_size(px(14.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(icon(Icon::Phone, px(15.), theme.text))
                .child(tr("ui.call_panel.native_call_experimental"))
                .child(div().flex_1())
                .when_some(status, |d, s| d.child(widgets::muted(s, 11., &theme))),
        )
        .child(body)
        .when_some(part.error.clone(), |d, error| {
            d.child(
                div()
                    .text_size(px(12.))
                    .text_color(theme.danger)
                    .child(error),
            )
        })
        .when(this.demo && part.active.is_none(), |d| {
            let link = |id: &'static str, label: String, video: bool| {
                div()
                    .id(id)
                    .text_color(theme.accent_text)
                    .cursor_pointer()
                    .on_click(cx.listener(move |this, _, _, _| {
                        send(this, CallAction::SimulateIncoming { video })
                    }))
                    .child(label)
            };
            d.child(
                div()
                    .flex()
                    .gap(px(12.))
                    .child(link(
                        "panel-incoming",
                        tr("ui.call_panel.simulate_incoming_call"),
                        false,
                    ))
                    .child(link(
                        "panel-incoming-video",
                        tr("ui.call_panel.video_2"),
                        true,
                    )),
            )
        })
        .into_any_element()
}

fn active_box(this: &mut Oxide, call: &ActiveCallView, cx: &mut Context<Oxide>) -> AnyElement {
    let theme = this.theme;
    let busy = matches!(
        call.phase,
        CallPhaseView::Preparing | CallPhaseView::Connecting | CallPhaseView::Ringing
    );
    let muted = call.muted;
    let controls = if call.phase == CallPhaseView::Ended {
        widgets::button(
            "panel-close",
            tr("ui.call_panel.close"),
            ButtonKind::Outline,
            &theme,
        )
        .on_click(cx.listener(|this, _, _, _| send(this, CallAction::Dismiss)))
        .into_any_element()
    } else {
        div()
            .flex()
            .gap(px(6.))
            .child(
                widgets::icon_button(
                    "panel-mute",
                    if muted { Icon::MicOff } else { Icon::Mic },
                    &theme,
                )
                .tooltip(widgets::tooltip(
                    if muted {
                        tr("ui.call_panel.unmute")
                    } else {
                        tr("ui.call_panel.mute")
                    },
                    theme,
                ))
                .on_click(cx.listener(move |this, _, _, _| {
                    send(this, CallAction::SetMuted { muted: !muted })
                })),
            )
            .child(
                div()
                    .id("panel-hangup")
                    .w(px(44.))
                    .h(px(32.))
                    .rounded(px(8.))
                    .bg(theme.danger)
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .tooltip(widgets::tooltip(tr("ui.call_panel.hang_up"), theme))
                    .on_click(cx.listener(|this, _, _, _| send(this, CallAction::Hangup)))
                    .child(icon(Icon::PhoneOff, px(15.), gpui::white())),
            )
            .into_any_element()
    };
    let media = &this.ui.call.media;
    let videos = (call.video && call.phase != CallPhaseView::Ended).then(|| {
        let tile = |feed: Option<&std::rc::Rc<media_video::VideoFeed>>, placeholder: String| {
            div()
                .h_full()
                .rounded(px(10.))
                .overflow_hidden()
                .bg(gpui::black())
                .flex()
                .items_center()
                .justify_center()
                .map(|d| match feed {
                    Some(feed) => d.child(media_video::view(&feed.slot, false)),
                    None => d.child(
                        div()
                            .text_size(px(12.))
                            .text_color(gpui::white().opacity(0.7))
                            .child(placeholder),
                    ),
                })
        };
        div()
            .h(px(180.))
            .flex()
            .gap(px(8.))
            .child(tile(media.remote_video.as_ref(), call.title.clone()).flex_1())
            .child(tile(media.local_video.as_ref(), tr("ui.call_panel.you")).w(px(160.)))
    });
    let people = (!call.participants.is_empty()).then(|| {
        let names: Vec<&str> = call.participants.iter().map(|p| p.name.as_str()).collect();
        widgets::muted(
            trf(
                "ui.call_panel.participant_s",
                &[("participantsCount", &call.participants.len().to_string())],
            ) + &names.join(", "),
            12.,
            &theme,
        )
    });
    div()
        .flex()
        .flex_col()
        .gap(px(10.))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(12.))
                .child(this.avatar(&call.id, &call.title, 40., AvatarShape::Round, None))
                .child(
                    div()
                        .flex_1()
                        .flex()
                        .flex_col()
                        .gap(px(3.))
                        .child(
                            div()
                                .text_size(px(15.))
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(call.title.clone()),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .text_size(px(12.))
                                .text_color(theme.muted)
                                .when(busy, |d| {
                                    d.child(widgets::spinner("panel-busy", 11., theme.muted))
                                })
                                .child(call.status.clone()),
                        ),
                )
                .child(controls),
        )
        .children(videos)
        .children(people)
        .into_any_element()
}

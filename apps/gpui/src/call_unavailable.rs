//! Calls where LiveKit's WebRTC build is not linked (it is the macOS framework, called through Objective-C):
//! the screens that would hold a call say it is not available, and the core is told the media failed,
//! so a call started elsewhere in the app ends cleanly instead of waiting.
use crate::oxide::{Oxide, text};
use gpui::{AnyElement, Context, Window, div, prelude::*, px};
use teams_core::{Action, CallAction, Effect};

pub struct Ui;

impl Ui {
    pub fn new(_window: &mut Window, _cx: &mut Context<Oxide>) -> Self {
        Self
    }
}

fn unavailable() -> String {
    text(
        "Les appels ne sont pas encore disponibles sur cette plateforme.",
        "Calls are not available on this platform yet.",
    )
    .to_owned()
}

/// A pre-join screen has nowhere to show: it is cancelled. An incoming call rings out on its own.
pub fn call_changed(this: &mut Oxide, _cx: &mut Context<Oxide>) {
    if this.call.as_ref().is_some_and(|p| p.prejoin.is_some()) {
        this.send(Action::Call {
            action: CallAction::CancelJoin,
        });
    }
}

/// The media the core asks for cannot be made: it hears so at once.
pub fn media_effect(this: &mut Oxide, effect: Effect, _cx: &mut Context<Oxide>) {
    if let Effect::MediaPrepare { call_id, .. } = effect {
        this.send(Action::Call {
            action: CallAction::MediaFailed {
                call_id,
                reason: unavailable(),
            },
        });
    }
}

pub fn overlay(_this: &mut Oxide, _window: &mut Window, _cx: &mut Context<Oxide>) -> Option<AnyElement> {
    None
}

pub fn debug_step(_this: &mut Oxide, _step: &str, _window: &mut Window, _cx: &mut Context<Oxide>) -> bool {
    false
}

fn note(this: &Oxide) -> AnyElement {
    div()
        .p(px(16.))
        .text_sm()
        .text_color(this.theme.muted)
        .child(unavailable())
        .into_any_element()
}

/// Settings › Devices.
pub fn device_settings(this: &mut Oxide, _window: &mut Window, _cx: &mut Context<Oxide>) -> AnyElement {
    note(this)
}

/// The calls screen's panel, where a call would be started.
pub fn panel(this: &mut Oxide, _window: &mut Window, _cx: &mut Context<Oxide>) -> AnyElement {
    note(this)
}

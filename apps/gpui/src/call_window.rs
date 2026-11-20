//! Live calls (`CallWindow.swift`): a window of its own for the pre-join screen, the incoming call, the call and the
//! test call report, open while the core reports either; plus the hooks that keep the media engine, the ringing and
//! the ringback in step with the core, as `Store.swift` does.
use crate::call_alert::{IncomingAlert, Ringback};
use crate::call_devices::{self, DeviceKind};
use crate::call_prefs::CallPrefs;
use crate::call_report::{Context as ReportContext, Report};
use crate::call_share_picker::{self, ScreenPicker};
use crate::call_stage::{self, CallLayout, LayoutState, TileMenu};
use crate::icons::{Icon, icon};
use crate::media_devices::{self, Devices, MediaKind, permissions};
use crate::media_engine::{self, Media, Window as Target};
use crate::media_tester::{self, CameraPreview, MicrophoneTester, Owner, SpeakerTester};
use crate::media_video;
use crate::oxide::{FONT, Oxide, tr, trf, trp};
use crate::theme::Theme;
use crate::widgets::{self, ButtonKind};
use futures::StreamExt;
use gpui::{
    Animation, AnimationExt, AnyElement, App, Bounds, Context, Corner, FocusHandle, FontWeight,
    Hsla, KeyDownEvent, Render, SharedString, Task, TitlebarOptions, WeakEntity, Window,
    WindowBounds, WindowHandle, WindowOptions, anchored, deferred, div, prelude::*, px, rgb, size,
};
use std::time::{Duration, Instant, SystemTime};
use teams_core::{
    Action, ActiveCallView, AvatarShape, CallAction, CallPhaseView, NotificationsAction,
    PrejoinView, RecordingView,
};

/// macOS's system green, for Accept.
fn green(theme: &Theme) -> Hsla {
    rgb(if theme.dark { 0x30D158 } else { 0x34C759 }).into()
}

pub struct Ui {
    pub media: Media,
    window: Option<WindowHandle<CallView>>,
    opening: bool,
    /// Redraws the clock once a second while the window is open.
    ticker: Option<Task<()>>,
    pub prefs: CallPrefs,
    pub devices: Devices,
    pub devices_loaded: bool,
    pub settings_mic: MicrophoneTester,
    pub prejoin_mic: MicrophoneTester,
    pub speaker_test: SpeakerTester,
    pub settings_preview: Option<CameraPreview>,
    pub settings_camera_denied: bool,
    /// When the Settings page last drew, and the task that stops its tests once it no longer does.
    pub settings_seen: Option<Instant>,
    pub settings_watch: Option<Task<()>>,
    pub prejoin_preview: Option<CameraPreview>,
    prejoin_camera_denied: bool,
    prejoin_shown: bool,
    alert: IncomingAlert,
    ringback: Ringback,
    pub layout: LayoutState,
    layout_call: Option<String>,
    show_roster: bool,
    pub picker: Option<ScreenPicker>,
    local_recording_error: Option<String>,
    pub tile_menu: Option<TileMenu>,
    pub active_speaker: Option<String>,
    pub speaking_now: Vec<String>,
    pub speaker_timer: Option<Task<()>>,
    /// The test report of the ended test call, made once.
    report: Option<(String, Report)>,
    /// The Calls screen's target field (`call_panel`).
    pub panel_target: gpui::Entity<crate::input::TextInput>,
}

impl Ui {
    pub fn new(_window: &mut Window, cx: &mut Context<Oxide>) -> Self {
        let (tx, mut rx) = futures::channel::mpsc::unbounded();
        cx.spawn(async move |this, cx| {
            while let Some(msg) = rx.next().await {
                if this
                    .update(cx, |this, cx| media_engine::on_msg(this, msg, cx))
                    .is_err()
                {
                    return;
                }
            }
        })
        .detach();
        let prefs = CallPrefs::load();
        Self {
            media: Media::new(tx),
            window: None,
            opening: false,
            ticker: None,
            layout: LayoutState::new(&prefs.layout),
            prefs,
            devices: Devices::default(),
            devices_loaded: false,
            settings_mic: MicrophoneTester::new(Owner::Settings),
            prejoin_mic: MicrophoneTester::new(Owner::Prejoin),
            speaker_test: SpeakerTester::default(),
            settings_preview: None,
            settings_camera_denied: false,
            settings_seen: None,
            settings_watch: None,
            prejoin_preview: None,
            prejoin_camera_denied: false,
            prejoin_shown: false,
            alert: IncomingAlert::default(),
            ringback: Ringback::default(),
            layout_call: None,
            show_roster: true,
            picker: None,
            local_recording_error: None,
            tile_menu: None,
            active_speaker: None,
            speaking_now: Vec::new(),
            speaker_timer: None,
            report: None,
            panel_target: crate::call_panel::target_field(cx),
        }
    }
}

/// Redraws the call window only (a video frame, the clock), not the main one.
pub fn redraw(this: &mut Oxide, cx: &mut Context<Oxide>) {
    if let Some(handle) = this.ui.call.window {
        let _ = handle.update(cx, |_, _, cx| cx.notify());
    }
}

/// The core's `CallPart` changed: window, ringing and ringback follow (`callWindow.sync`, `incomingAlert.sync`,
/// `ringback.sync`).
pub fn call_changed(this: &mut Oxide, cx: &mut Context<Oxide>) {
    let part = this.call.clone();
    let call_sounds = this.notifications.as_ref().is_none_or(|n| n.call_sounds);
    let prefs = this.prefs.clone();
    this.ui.call.alert.sync(part.as_ref(), call_sounds, &prefs);
    this.ui
        .call
        .ringback
        .sync(part.as_ref(), call_sounds, &prefs);

    // A new call starts with the remembered layout and nothing pinned, as a new `InCallView` would.
    let call_id = part
        .as_ref()
        .and_then(|p| p.active.as_ref())
        .map(|c| c.id.clone());
    if call_id.is_some() && call_id != this.ui.call.layout_call {
        let ui = &mut this.ui.call;
        ui.layout = LayoutState::new(&ui.prefs.layout);
        ui.layout_call = call_id.clone();
        ui.active_speaker = None;
        ui.speaking_now.clear();
        ui.speaker_timer = None;
        ui.show_roster = true;
        ui.local_recording_error = None;
        ui.report = None;
    }

    // The pre-join screen meters the microphone while it shows, and previews the camera when on.
    let prejoin = part.as_ref().is_some_and(|p| p.prejoin.is_some());
    if prejoin && !this.ui.call.prejoin_shown {
        this.ui.call.prejoin_shown = true;
        media_tester::start_mic(this, Owner::Prejoin, cx);
        if this.ui.call.prefs.prejoin_camera {
            start_prejoin_preview(this, cx);
        }
    } else if !prejoin && this.ui.call.prejoin_shown {
        this.ui.call.prejoin_shown = false;
        media_tester::stop_mic(this, Owner::Prejoin);
        this.ui.call.prejoin_preview = None;
    }

    let visible = part
        .as_ref()
        .is_some_and(|p| p.prejoin.is_some() || p.active.is_some());
    if !visible {
        close_window(this, cx);
        return;
    }
    let title = part
        .as_ref()
        .and_then(|p| {
            p.active
                .as_ref()
                .map(|c| c.title.clone())
                .or_else(|| p.prejoin.as_ref().map(|p| p.title.clone()))
        })
        .unwrap_or_else(|| tr("ui.call_window.call"));
    let incoming = part
        .as_ref()
        .and_then(|p| p.active.as_ref())
        .is_some_and(|c| c.phase == CallPhaseView::Incoming);
    match this.ui.call.window {
        Some(handle) => {
            let _ = handle.update(cx, |_, window, cx| {
                window.set_window_title(&title);
                if incoming {
                    window.activate_window();
                }
                cx.notify();
            });
        }
        None => open_window(this, title, cx),
    }
}

fn open_window(this: &mut Oxide, title: String, cx: &mut Context<Oxide>) {
    if this.ui.call.opening {
        return;
    }
    this.ui.call.opening = true;
    let oxide = cx.entity().downgrade();
    // Not while the store is being updated: a new window draws at once, and its view reads the store.
    cx.defer(move |cx| {
        let mut bounds = Bounds::centered(None, size(px(880.), px(600.)), cx);
        // Scripted runs (`OXIDE_SCRIPT`): beside the main window, which would cover it and stop its drawing.
        if std::env::var_os("OXIDE_SCRIPT").is_some() {
            bounds.origin.x += px(420.);
        }
        let view = oxide.clone();
        let handle = cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(560.), px(420.))),
                titlebar: Some(TitlebarOptions {
                    title: Some(title.into()),
                    ..Default::default()
                }),
                focus: true,
                show: true,
                ..Default::default()
            },
            |window, cx| cx.new(|cx| CallView::new(view, window, cx)),
        );
        let _ = oxide.update(cx, |this, cx| {
            this.ui.call.opening = false;
            this.ui.call.window = handle.ok();
            start_ticker(this, cx);
            // The call may have ended while the window opened.
            if !this
                .call
                .as_ref()
                .is_some_and(|p| p.prejoin.is_some() || p.active.is_some())
            {
                close_window(this, cx);
            }
        });
    });
}

fn close_window(this: &mut Oxide, cx: &mut Context<Oxide>) {
    let ui = &mut this.ui.call;
    ui.ticker = None;
    call_share_picker::cancel(this);
    let ui = &mut this.ui.call;
    ui.tile_menu = None;
    let Some(handle) = ui.window.take() else {
        return;
    };
    cx.defer(move |cx| {
        let _ = handle.update(cx, |_, window, _| window.remove_window());
    });
}

/// The elapsed time and the local recording's clock move every second.
fn start_ticker(this: &mut Oxide, cx: &mut Context<Oxide>) {
    this.ui.call.ticker = Some(cx.spawn(async move |this, cx| {
        loop {
            cx.background_executor().timer(Duration::from_secs(1)).await;
            if this.update(cx, redraw).is_err() {
                return;
            }
        }
    }));
}

/// Every `Media*` effect.
pub fn media_effect(this: &mut Oxide, effect: teams_core::Effect, cx: &mut Context<Oxide>) {
    media_engine::perform(this, effect, cx);
    redraw(this, cx);
}

/// Dialogs and floating panels of this module, drawn over the main window: none, the call has its own window.
pub fn overlay(
    this: &mut Oxide,
    window: &mut Window,
    cx: &mut Context<Oxide>,
) -> Option<AnyElement> {
    let _ = (this, window, cx);
    None
}

fn send(this: &Oxide, action: CallAction) {
    this.send(Action::Call { action });
}

/// Closing the window cancels the pre-join, leaves a running call, or dismisses an ended one.
fn window_closing(this: &mut Oxide) {
    let part = this.call.clone();
    if part.as_ref().is_some_and(|p| p.prejoin.is_some()) {
        send(this, CallAction::CancelJoin);
    } else if let Some(call) = part.as_ref().and_then(|p| p.active.as_ref()) {
        send(
            this,
            if call.phase == CallPhaseView::Ended {
                CallAction::Dismiss
            } else {
                CallAction::Hangup
            },
        );
    }
    this.ui.call.window = None;
    this.ui.call.ticker = None;
    call_share_picker::cancel(this);
}

/// The call window's root view: it draws from the store.
pub struct CallView {
    oxide: WeakEntity<Oxide>,
    focus: FocusHandle,
}

impl CallView {
    fn new(oxide: WeakEntity<Oxide>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        window.focus(&focus);
        let closing = oxide.clone();
        window.on_window_should_close(cx, move |_, cx| {
            let _ = closing.update(cx, |this, _| window_closing(this));
            true
        });
        Self { oxide, focus }
    }
}

impl Render for CallView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(oxide) = self.oxide.upgrade() else {
            return div().into_any_element();
        };
        let keys = self.oxide.clone();
        let content = oxide.update(cx, |this, cx| root(this, window, cx));
        div()
            .size_full()
            .track_focus(&self.focus)
            .on_key_down(move |event: &KeyDownEvent, window, cx| {
                let _ = keys.update(cx, |this, cx| key_down(this, event, window, cx));
            })
            .child(content)
            .into_any_element()
    }
}

/// Keyboard shortcuts of the call window (the macOS app's `keyboardShortcut`s).
fn key_down(this: &mut Oxide, event: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Oxide>) {
    let Some(part) = this.call.clone() else {
        return;
    };
    let keystroke = &event.keystroke;
    let m = &keystroke.modifiers;
    let key = keystroke.key.as_str();
    let command_shift = m.platform && m.shift && !m.control && !m.alt;
    if let Some(prejoin) = &part.prejoin {
        match key {
            "escape" => send(this, CallAction::CancelJoin),
            "enter" if prejoin.other_devices == 0 => join(this, false),
            "enter" => join(this, true),
            _ => return,
        }
    } else if let Some(call) = &part.active {
        match (key, call.phase) {
            ("escape", CallPhaseView::Incoming) => send(this, CallAction::Decline),
            ("enter", CallPhaseView::Incoming) if call.can_accept => {
                send(this, CallAction::Accept { video: false })
            }
            ("enter", CallPhaseView::Ended) => send(this, CallAction::Dismiss),
            ("m", _) if command_shift && call.phase != CallPhaseView::Ended => {
                send(this, CallAction::SetMuted { muted: !call.muted })
            }
            ("o", _) if command_shift && (call.phase == CallPhaseView::Connected || call.video) => {
                send(this, CallAction::SetCamera { on: !call.camera })
            }
            ("e", CallPhaseView::Connected) if command_shift => toggle_share(this, call),
            _ => return,
        }
    }
    cx.stop_propagation();
    redraw(this, cx);
}

fn root(this: &mut Oxide, window: &mut Window, cx: &mut Context<Oxide>) -> AnyElement {
    let theme = this.theme;
    let part = this.call.clone().unwrap_or_default();
    let body = if let Some(prejoin) = &part.prejoin {
        prejoin_view(this, prejoin, part.error.clone(), window, cx)
    } else if let Some(call) = part
        .active
        .as_ref()
        .filter(|c| c.is_test && c.phase == CallPhaseView::Ended)
    {
        report_view(this, call, cx)
    } else if let Some(call) = part
        .active
        .as_ref()
        .filter(|c| c.phase == CallPhaseView::Incoming)
    {
        incoming_view(this, call, cx)
    } else if let Some(call) = &part.active {
        in_call_view(this, call, part.error.clone(), window, cx)
    } else {
        div().into_any_element()
    };
    let picker = call_share_picker::view(this, window, cx);
    div()
        .size_full()
        .relative()
        .bg(theme.background)
        .font_family(FONT)
        .text_size(px(13.))
        .text_color(theme.text)
        .child(body)
        .children(picker)
        .into_any_element()
}

// MARK: Pieces

/// The toggle buttons of the control bar: tinted when on, grey when off.
fn toggle(
    id: &'static str,
    on: bool,
    glyph: Icon,
    label: String,
    theme: &Theme,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .w(px(44.))
        .h(px(38.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(10.))
        .cursor_pointer()
        .bg(if on { theme.input } else { theme.muted })
        .child(icon(
            glyph,
            px(16.),
            if on { theme.text } else { gpui::white() },
        ))
        .tooltip(widgets::tooltip(label, *theme))
}

fn faded(button: gpui::Stateful<gpui::Div>, disabled: bool) -> gpui::Stateful<gpui::Div> {
    if disabled {
        button.opacity(0.45).cursor_default()
    } else {
        button
    }
}

fn banner(glyph: Icon, text: String, tint: Hsla, theme: &Theme) -> AnyElement {
    div()
        .flex_none()
        .flex()
        .items_center()
        .gap(px(8.))
        .px(px(16.))
        .py(px(8.))
        .bg(tint)
        .text_size(px(12.))
        .child(icon(glyph, px(14.), theme.text))
        .child(div().flex_1().child(text))
        .into_any_element()
}

/// A local or remote video in a black box, or a placeholder text before it.
fn video_tile(
    feed: Option<&std::rc::Rc<media_video::VideoFeed>>,
    placeholder: String,
    theme: &Theme,
) -> AnyElement {
    let _ = theme;
    div()
        .size_full()
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
        .into_any_element()
}

/// `CallClock.elapsed`: `mm:ss`, or `h:mm:ss` past an hour.
pub fn elapsed(since_ms: i64, now: SystemTime) -> String {
    let now = now
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    let seconds = (now - since_ms / 1000).max(0);
    let (h, m, s) = (seconds / 3600, (seconds % 3600) / 60, seconds % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

/// `CallClock.role`.
fn role(role: &str) -> Option<String> {
    match role {
        "organizer" => Some(tr("ui.call_window.organizer")),
        "presenter" => Some(tr("ui.call_window.presenter")),
        "attendee" => Some(tr("ui.call_window.attendee")),
        _ => None,
    }
}

// MARK: Pre-join

fn start_prejoin_preview(_this: &mut Oxide, cx: &mut Context<Oxide>) {
    cx.spawn(async move |this, cx| {
        let granted = permissions::request(MediaKind::Video).await;
        let _ = this.update(cx, |this, cx| {
            let ui = &mut this.ui.call;
            ui.prejoin_camera_denied = !granted;
            ui.prefs.prejoin_camera = granted;
            ui.prefs.save();
            if granted
                && ui.prejoin_shown
                && ui.prejoin_preview.is_none()
                && let Some(mut preview) = CameraPreview::new(Target::Call, ui.media.tx.clone())
            {
                preview.show(media_devices::resolved_camera(&this.prefs));
                this.ui.call.prejoin_preview = Some(preview);
            }
            redraw(this, cx);
        });
    })
    .detach();
}

fn join(this: &mut Oxide, transfer: bool) {
    media_tester::stop_mic(this, Owner::Prejoin);
    let ui = &this.ui.call;
    let video = ui.prefs.prejoin_camera && !ui.prejoin_camera_denied;
    let muted = ui.prefs.prejoin_muted;
    send(
        this,
        CallAction::ConfirmJoin {
            video,
            muted,
            speaker: true,
            transfer,
        },
    );
}

fn device_choice(
    this: &mut Oxide,
    title: String,
    glyph: Icon,
    kind: DeviceKind,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    let theme = this.theme;
    div()
        .flex()
        .flex_col()
        .gap(px(4.))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(5.))
                .text_size(px(12.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_color(theme.muted)
                .child(icon(glyph, px(13.), theme.muted))
                .child(title),
        )
        .child(call_devices::picker(this, kind, false, cx))
        .into_any_element()
}

fn prejoin_view(
    this: &mut Oxide,
    prejoin: &PrejoinView,
    error: Option<String>,
    _window: &mut Window,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    let theme = this.theme;
    let ui = &this.ui.call;
    let (camera, muted, denied) = (
        ui.prefs.prejoin_camera,
        ui.prefs.prejoin_muted,
        ui.prejoin_camera_denied,
    );
    let (level, metering) = (ui.prejoin_mic.level, ui.prejoin_mic.running);
    let preview = (camera && !denied)
        .then(|| {
            ui.prejoin_preview
                .as_ref()
                .map(|p| media_video::view(&p.slot, false))
        })
        .flatten();
    let (me, name) = this
        .shell
        .as_ref()
        .map_or(("me".to_owned(), tr("ui.call_window.you")), |s| {
            (s.me.clone(), s.display_name.clone())
        });
    let video_box = div()
        .w_full()
        .h(px(270.))
        .rounded(px(14.))
        .overflow_hidden()
        .bg(gpui::black())
        .flex()
        .items_center()
        .justify_center()
        .map(|d| match preview {
            Some(video) => d.child(video),
            None => d.child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(10.))
                    .child(this.avatar(&me, &name, 72., AvatarShape::Round, None))
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(gpui::white().opacity(0.7))
                            .child(if denied {
                                tr("ui.call_window.camera_access_denied")
                            } else {
                                tr("ui.call_window.camera_is_off")
                            }),
                    ),
            ),
        });
    let toggles = div()
        .flex()
        .items_center()
        .gap(px(10.))
        .child(
            toggle(
                "prejoin-mic",
                !muted,
                if muted { Icon::MicOff } else { Icon::Mic },
                if muted {
                    tr("ui.call_window.mic_is_off")
                } else {
                    tr("ui.call_window.mic_is_on")
                },
                &theme,
            )
            .on_click(cx.listener(|this, _, _, cx| {
                let prefs = &mut this.ui.call.prefs;
                prefs.prejoin_muted = !prefs.prejoin_muted;
                prefs.save();
                redraw(this, cx);
            })),
        )
        .child(
            toggle(
                "prejoin-camera",
                camera,
                if camera { Icon::Video } else { Icon::VideoOff },
                if camera {
                    tr("ui.call_window.camera_is_on")
                } else {
                    tr("ui.call_window.camera_is_off")
                },
                &theme,
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                if camera {
                    this.ui.call.prefs.prejoin_camera = false;
                    this.ui.call.prefs.save();
                    this.ui.call.prejoin_preview = None;
                    redraw(this, cx);
                } else {
                    start_prejoin_preview(this, cx);
                }
            })),
        )
        .child(
            div()
                .id("prejoin-level")
                .when(muted, |d| d.opacity(0.3))
                .tooltip(widgets::tooltip(tr("ui.call_window.mic_level"), theme))
                .child(call_devices::level_meter(level, metering, 90., &theme)),
        );
    let left = div()
        .flex()
        .flex_col()
        .items_center()
        .gap(px(14.))
        .flex_1()
        .max_w(px(480.))
        .child(video_box)
        .child(toggles);

    let other_devices = prejoin.other_devices > 0;
    let actions = if other_devices {
        div()
            .flex()
            .flex_col()
            .gap(px(10.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(icon(Icon::Devices, px(16.), theme.text))
                    .child(tr("ui.call_window.you_re_already_in_this_meeting")),
            )
            .child(
                widgets::button(
                    "prejoin-transfer",
                    tr("ui.call_window.transfer_to_this_device"),
                    ButtonKind::Primary,
                    &theme,
                )
                .w_full()
                .tooltip(widgets::tooltip(
                    tr("ui.call_window.the_other_device_leaves_the_meeting"),
                    theme,
                ))
                .on_click(cx.listener(|this, _, _, _| join(this, true))),
            )
            .child(
                widgets::button(
                    "prejoin-add",
                    tr("ui.call_window.add_this_device"),
                    ButtonKind::Outline,
                    &theme,
                )
                .w_full()
                .tooltip(widgets::tooltip(
                    tr("ui.call_window.join_without_mic_camera_or_audio"),
                    theme,
                ))
                .on_click(cx.listener(|this, _, _, _| {
                    media_tester::stop_mic(this, Owner::Prejoin);
                    send(
                        this,
                        CallAction::ConfirmJoin {
                            video: false,
                            muted: true,
                            speaker: false,
                            transfer: false,
                        },
                    );
                })),
            )
            .child(
                div()
                    .id("prejoin-cancel-link")
                    .text_color(theme.accent_text)
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, _| send(this, CallAction::CancelJoin)))
                    .child(tr("ui.call_window.cancel")),
            )
            .into_any_element()
    } else {
        div()
            .flex()
            .justify_between()
            .child(
                widgets::button(
                    "prejoin-cancel",
                    tr("ui.call_window.cancel"),
                    ButtonKind::Outline,
                    &theme,
                )
                .on_click(cx.listener(|this, _, _, _| send(this, CallAction::CancelJoin))),
            )
            .child(
                widgets::button(
                    "prejoin-join",
                    tr("ui.call_window.join_now"),
                    ButtonKind::Primary,
                    &theme,
                )
                .on_click(cx.listener(|this, _, _, _| join(this, false))),
            )
            .into_any_element()
    };
    let right = div()
        .w(px(300.))
        .h_full()
        .flex_none()
        .flex()
        .flex_col()
        .gap(px(16.))
        .child(
            div()
                .text_size(px(22.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(prejoin.title.clone()),
        )
        .child(widgets::muted(
            tr("ui.call_window.choose_your_audio_and_video_settings"),
            13.,
            &theme,
        ))
        .child(device_choice(
            this,
            tr("ui.call_window.microphone"),
            Icon::Mic,
            DeviceKind::Microphone,
            cx,
        ))
        .child(device_choice(
            this,
            tr("ui.call_window.speaker"),
            Icon::Speaker,
            DeviceKind::Speaker,
            cx,
        ))
        .child(device_choice(
            this,
            tr("ui.call_window.camera"),
            Icon::Video,
            DeviceKind::Camera,
            cx,
        ))
        .when_some(error, |d, error| {
            d.child(
                div()
                    .text_size(px(12.))
                    .text_color(theme.danger)
                    .child(error),
            )
        })
        .child(div().flex_1())
        .child(actions);
    div()
        .size_full()
        .p(px(28.))
        .flex()
        .items_start()
        .justify_center()
        .gap(px(28.))
        .child(left)
        .child(right)
        .into_any_element()
}

// MARK: Test call report

fn report_for(this: &mut Oxide, call: &ActiveCallView) -> Report {
    if let Some((id, report)) = &this.ui.call.report
        && *id == call.id
    {
        return report.clone();
    }
    let speaker = media_devices::resolved_speaker(&this.prefs);
    let context = ReportContext {
        microphones: media_devices::core_audio::devices(true).len(),
        speakers: media_devices::core_audio::devices(false).len(),
        muted_at_end: call.muted,
        output_muted: speaker.is_some_and(media_devices::core_audio::is_muted),
        has_camera: media_devices::resolved_camera(&this.prefs).is_some(),
    };
    let report = Report::make(&this.ui.call.media.diagnostics, &context);
    this.ui.call.report = Some((call.id.clone(), report.clone()));
    report
}

fn report_row(title: String, glyph: Icon, problems: &[String], theme: &Theme) -> AnyElement {
    let ok = green(theme);
    div()
        .p(px(14.))
        .flex()
        .items_start()
        .gap(px(12.))
        .child(
            div()
                .w(px(20.))
                .flex_none()
                .child(icon(glyph, px(16.), theme.muted)),
        )
        .child(
            div()
                .w(px(110.))
                .flex_none()
                .text_size(px(14.))
                .font_weight(FontWeight::SEMIBOLD)
                .child(title),
        )
        .child(
            div()
                .flex_1()
                .flex()
                .flex_col()
                .gap(px(4.))
                .text_size(px(13.))
                .map(|d| {
                    if problems.is_empty() {
                        d.child(
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .text_color(ok)
                                .child(icon(Icon::Check, px(14.), ok))
                                .child("OK"),
                        )
                    } else {
                        d.children(problems.iter().map(|p| {
                            div()
                                .flex()
                                .items_center()
                                .gap(px(6.))
                                .text_color(theme.danger)
                                .child(icon(Icon::Close, px(14.), theme.danger))
                                .child(p.clone())
                        }))
                    }
                }),
        )
        .into_any_element()
}

fn report_view(this: &mut Oxide, call: &ActiveCallView, cx: &mut Context<Oxide>) -> AnyElement {
    let theme = this.theme;
    let report = report_for(this, call);
    let clean = report.is_clean();
    let mark = if clean { green(&theme) } else { theme.danger };
    div()
        .size_full()
        .p(px(28.))
        .flex()
        .flex_col()
        .gap(px(18.))
        .child(
            div()
                .flex()
                .items_center()
                .gap(px(12.))
                .child(icon(
                    if clean { Icon::Check } else { Icon::Alert },
                    px(30.),
                    mark,
                ))
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(3.))
                        .child(widgets::title(tr("ui.call_window.test_call_results"), 20.))
                        .child(widgets::muted(
                            if clean {
                                tr("ui.call_window.no_problems_detected_did_you_hear")
                            } else {
                                call.status.clone()
                            },
                            13.,
                            &theme,
                        )),
                ),
        )
        .child(
            widgets::card(&theme)
                .flex()
                .flex_col()
                .child(report_row(
                    tr("ui.call_window.microphone"),
                    Icon::Mic,
                    &report.microphone,
                    &theme,
                ))
                .child(widgets::divider(&theme))
                .child(report_row(
                    tr("ui.call_window.speaker"),
                    Icon::Speaker,
                    &report.speaker,
                    &theme,
                ))
                .child(widgets::divider(&theme))
                .child(report_row(
                    tr("ui.call_window.camera"),
                    Icon::Video,
                    &report.camera,
                    &theme,
                ))
                .child(widgets::divider(&theme))
                .child(report_row(
                    tr("ui.call_window.network"),
                    Icon::Globe,
                    &report.network,
                    &theme,
                )),
        )
        .child(widgets::muted(
            tr("ui.call_window.this_report_lists_the_problems_detected"),
            11.,
            &theme,
        ))
        .child(div().flex_1())
        .child(
            div()
                .flex()
                .gap(px(8.))
                .child(
                    widgets::button(
                        "report-settings",
                        tr("ui.call_window.audio_and_video_settings"),
                        ButtonKind::Outline,
                        &theme,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        open_audio_settings(this, cx);
                        send(this, CallAction::Dismiss);
                    })),
                )
                .child(div().flex_1())
                .child(
                    widgets::button(
                        "report-again",
                        tr("ui.call_window.test_again"),
                        ButtonKind::Outline,
                        &theme,
                    )
                    .on_click(cx.listener(|this, _, _, _| {
                        send(this, CallAction::Dismiss);
                        let bot_mri = this.ui.call.prefs.bot_override();
                        send(this, CallAction::TestCall { bot_mri });
                    })),
                )
                .child(
                    widgets::button(
                        "report-close",
                        tr("ui.call_window.close"),
                        ButtonKind::Primary,
                        &theme,
                    )
                    .on_click(cx.listener(|this, _, _, _| send(this, CallAction::Dismiss))),
                ),
        )
        .into_any_element()
}

/// Settings › Audio and video in the main window (`store.openSettings(.audioVideo)`).
fn open_audio_settings(this: &mut Oxide, cx: &mut Context<Oxide>) {
    crate::settings::open(this, crate::settings::Page::AudioVideo, cx);
    // The main window: the one that is not the call's.
    if let Some(main) = cx
        .windows()
        .into_iter()
        .find(|w| w.downcast::<CallView>().is_none())
    {
        let _ = main.update(cx, |_, window, _| window.activate_window());
    }
}

// MARK: Incoming

fn answer_button(
    id: &'static str,
    label: String,
    glyph: Icon,
    color: Hsla,
    enabled: bool,
) -> gpui::Stateful<gpui::Div> {
    div()
        .id(id)
        .h(px(42.))
        .px(px(18.))
        .flex()
        .items_center()
        .gap(px(8.))
        .rounded(px(12.))
        .bg(color)
        .text_size(px(14.))
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(gpui::white())
        .child(icon(glyph, px(16.), gpui::white()))
        .child(label)
        .map(|d| {
            if enabled {
                d.cursor_pointer().hover(|s| s.opacity(0.9))
            } else {
                d.opacity(0.45)
            }
        })
}

fn incoming_view(this: &mut Oxide, call: &ActiveCallView, cx: &mut Context<Oxide>) -> AnyElement {
    let theme = this.theme;
    let green = green(&theme);
    let can_accept = call.can_accept;
    div()
        .size_full()
        .p(px(28.))
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(18.))
        .child(this.avatar(&call.id, &call.title, 96., AvatarShape::Round, None))
        .child(
            div()
                .text_size(px(24.))
                .font_weight(FontWeight::SEMIBOLD)
                .text_center()
                .child(call.title.clone()),
        )
        .child(widgets::muted(
            if call.offers_video {
                tr("ui.call_window.incoming_video_call")
            } else {
                tr("ui.call_window.incoming_audio_call")
            },
            14.,
            &theme,
        ))
        .when(!can_accept, |d| {
            d.child(widgets::spinner("incoming-wait", 14., theme.muted))
        })
        .child(
            div()
                .flex()
                .gap(px(16.))
                .child(
                    answer_button(
                        "decline",
                        tr("ui.call_window.decline"),
                        Icon::PhoneOff,
                        theme.danger,
                        true,
                    )
                    .on_click(cx.listener(|this, _, _, _| send(this, CallAction::Decline))),
                )
                .child(
                    answer_button(
                        "accept",
                        tr("ui.call_window.accept"),
                        Icon::Phone,
                        green,
                        can_accept,
                    )
                    .when(can_accept, |b| {
                        b.on_click(cx.listener(|this, _, _, _| {
                            send(this, CallAction::Accept { video: false })
                        }))
                    }),
                )
                .when(call.offers_video, |d| {
                    d.child(
                        answer_button(
                            "accept-video",
                            tr("ui.call_window.with_video"),
                            Icon::Video,
                            green,
                            can_accept,
                        )
                        .when(can_accept, |b| {
                            b.on_click(cx.listener(|this, _, _, _| {
                                send(this, CallAction::Accept { video: true })
                            }))
                        }),
                    )
                }),
        )
        .into_any_element()
}

// MARK: In call

/// Outgoing call not answered yet: the other side rings, or is about to.
fn calling(call: &ActiveCallView) -> bool {
    !call.is_meeting
        && !call.is_test
        && matches!(
            call.phase,
            CallPhaseView::Preparing | CallPhaseView::Connecting | CallPhaseView::Ringing
        )
}

fn placeholder(call: &ActiveCallView) -> String {
    if call.phase == CallPhaseView::Ended {
        return call.status.clone();
    }
    if call.is_meeting {
        let count = call.participants.iter().filter(|p| !p.in_lobby).count();
        return if count == 0 {
            tr("ui.call_window.no_one_else_is_here_yet")
        } else {
            trp("ui.call_window.other_participants", count as i64, &[])
        };
    }
    call.title.clone()
}

fn header(call: &ActiveCallView, theme: &Theme) -> AnyElement {
    let busy = matches!(
        call.phase,
        CallPhaseView::Preparing | CallPhaseView::Connecting | CallPhaseView::Ringing
    );
    let clock = call
        .connected_at_ms
        .filter(|_| call.phase == CallPhaseView::Connected);
    div()
        .flex_none()
        .px(px(18.))
        .py(px(12.))
        .bg(theme.surface)
        .flex()
        .flex_col()
        .gap(px(2.))
        .child(
            div()
                .text_size(px(16.))
                .font_weight(FontWeight::SEMIBOLD)
                .whitespace_nowrap()
                .overflow_hidden()
                .text_ellipsis()
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
                    d.child(widgets::spinner("call-busy", 11., theme.muted))
                })
                .when_some(clock, |d, start| {
                    d.child(elapsed(start, SystemTime::now()))
                        .child(div().text_color(theme.subtle).child("·"))
                })
                .child(
                    div()
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .text_ellipsis()
                        .child(call.status.clone()),
                ),
        )
        .into_any_element()
}

fn recording_banners(
    this: &Oxide,
    call: &ActiveCallView,
    cx: &mut Context<Oxide>,
) -> Vec<AnyElement> {
    let theme = this.theme;
    let mut out = Vec::new();
    match &call.recording {
        RecordingView::Active { by } => out.push(banner(
            Icon::Record,
            by.as_ref().map_or_else(
                || tr("ui.call_window.recording_in_teams"),
                |name| trf("ui.call_window.recording_started_by", &[("name", name)]),
            ),
            theme.error_background,
            &theme,
        )),
        RecordingView::Starting => out.push(banner(
            Icon::Record,
            tr("ui.call_window.starting_the_recording"),
            theme.accent_soft,
            &theme,
        )),
        RecordingView::Stopping => out.push(banner(
            Icon::Record,
            tr("ui.call_window.stopping_the_recording"),
            theme.accent_soft,
            &theme,
        )),
        RecordingView::Failed { reason } => out.push(banner(
            Icon::Alert,
            reason.clone(),
            theme.error_background,
            &theme,
        )),
        RecordingView::None => {}
    }
    let media = &this.ui.call.media;
    if let Some(started) = media.local_recording_started {
        let ms = started
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as i64);
        out.push(banner(
            Icon::Wave,
            format!(
                "{} · {}",
                tr("ui.call_window.recording_on_this_mac"),
                elapsed(ms, SystemTime::now())
            ),
            theme.error_background,
            &theme,
        ));
    }
    if let Some(error) = &this.ui.call.local_recording_error {
        out.push(banner(
            Icon::Alert,
            error.clone(),
            theme.error_background,
            &theme,
        ));
    }
    if let Some(saved) = &media.last_local_recording {
        let name = saved
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let reveal = saved.clone();
        out.push(
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap(px(8.))
                .px(px(16.))
                .py(px(8.))
                .bg(theme.accent_soft)
                .text_size(px(12.))
                .child(icon(Icon::Check, px(14.), theme.text))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .text_ellipsis()
                        .child(trf("ui.call_window.recording_saved", &[("name", &name)])),
                )
                .child(
                    div()
                        .id("reveal-recording")
                        .text_color(theme.accent_text)
                        .cursor_pointer()
                        .on_click(move |_, _, _| {
                            let _ = std::process::Command::new("open")
                                .arg("-R")
                                .arg(&reveal)
                                .spawn();
                        })
                        .child(tr("ui.call_window.show_in_finder")),
                )
                .child(
                    div()
                        .id("dismiss-recording")
                        .cursor_pointer()
                        .tooltip(widgets::tooltip(tr("ui.call_window.dismiss"), theme))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.ui.call.media.last_local_recording = None;
                            redraw(this, cx);
                        }))
                        .child(icon(Icon::Close, px(12.), theme.muted)),
                )
                .into_any_element(),
        );
    }
    out
}

fn start_local_recording(this: &mut Oxide, call: &ActiveCallView) {
    this.ui.call.local_recording_error = None;
    this.ui.call.media.last_local_recording = None;
    let folder = crate::media_recorder::folder(&this.ui.call.prefs.recordings_folder);
    let result = crate::media_recorder::default_path(&call.title, &folder)
        .and_then(|path| media_engine::start_local_recording(this, &call.id, path));
    if let Err(error) = result {
        this.ui.call.local_recording_error = Some(trf(
            "ui.call_window.could_not_record_on_this_mac",
            &[("error", &error)],
        ));
    }
}

fn toggle_share(this: &mut Oxide, call: &ActiveCallView) {
    if call.sharing_screen {
        send(this, CallAction::SetScreenShare { on: false });
    } else {
        call_share_picker::open(this);
    }
}

/// A control-bar menu opening upward from its button.
fn menu_up(
    items: Vec<AnyElement>,
    theme: &Theme,
    close: impl Fn(&mut Window, &mut App) + 'static,
) -> AnyElement {
    deferred(
        anchored()
            .anchor(Corner::BottomLeft)
            .snap_to_window_with_margin(px(8.))
            .child(
                div()
                    .occlude()
                    .mb_1()
                    .rounded(px(10.))
                    .border_1()
                    .border_color(theme.border)
                    .bg(theme.surface)
                    .shadow_lg()
                    .py_1()
                    .on_mouse_down_out(move |_, w, cx| close(w, cx))
                    .children(items),
            ),
    )
    .with_priority(1)
    .into_any_element()
}

fn closer(cx: &mut Context<Oxide>) -> impl Fn(&mut Window, &mut App) + 'static {
    let entity = cx.entity().downgrade();
    move |_, cx| {
        let _ = entity.update(cx, |this, cx| {
            this.close_popup(cx);
            redraw(this, cx);
        });
    }
}

fn record_menu(this: &mut Oxide, call: &ActiveCallView, cx: &mut Context<Oxide>) -> AnyElement {
    let theme = this.theme;
    let local = this.ui.call.media.local_recording_started.is_some();
    let cloud = matches!(
        call.recording,
        RecordingView::Starting | RecordingView::Active { .. } | RecordingView::Stopping
    );
    let on = local || cloud;
    let open = this.popup_open("call.record");
    let button = div()
        .id("call-record")
        .w(px(44.))
        .h(px(38.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(10.))
        .cursor_pointer()
        .bg(if on { theme.danger } else { theme.input })
        .child(icon(
            Icon::Record,
            px(16.),
            if on { gpui::white() } else { theme.text },
        ))
        .tooltip(widgets::tooltip(tr("ui.call_window.record"), theme))
        .on_click(cx.listener(|this, _, _, cx| {
            this.toggle_popup("call.record", cx);
            redraw(this, cx);
        }));
    let menu = open.then(|| {
        let mut items = Vec::new();
        let can_record = call.can_record;
        if cloud {
            let enabled = can_record && call.recording != RecordingView::Stopping;
            items.push(
                widgets::disabled(
                    widgets::menu_item(
                        "rec-cloud",
                        None,
                        tr("ui.call_window.stop_recording_in_teams"),
                        &theme,
                    ),
                    !enabled,
                )
                .when(enabled, |d| {
                    d.on_click(cx.listener(|this, _, _, cx| {
                        this.close_popup(cx);
                        send(this, CallAction::StopRecording);
                    }))
                })
                .into_any_element(),
            );
        } else {
            items.push(
                widgets::disabled(
                    widgets::menu_item(
                        "rec-cloud",
                        None,
                        tr("ui.call_window.record_in_teams"),
                        &theme,
                    ),
                    !can_record,
                )
                .when(can_record, |d| {
                    d.on_click(cx.listener(|this, _, _, cx| {
                        this.close_popup(cx);
                        send(this, CallAction::StartRecording);
                    }))
                })
                .into_any_element(),
            );
        }
        let call = call.clone();
        items.push(
            widgets::menu_item(
                "rec-local",
                None,
                if local {
                    tr("ui.call_window.stop_recording_on_this_mac")
                } else {
                    tr("ui.call_window.record_on_this_mac_audio")
                },
                &theme,
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.close_popup(cx);
                if local {
                    media_engine::stop_local_recording(&mut this.ui.call.media);
                } else {
                    start_local_recording(this, &call);
                }
                redraw(this, cx);
            }))
            .into_any_element(),
        );
        menu_up(items, &theme, closer(cx))
    });
    // The menu anchors at the button's top edge and opens upward.
    div()
        .flex()
        .flex_col()
        .children(menu)
        .child(button)
        .into_any_element()
}

fn layout_menu(this: &mut Oxide, call: &ActiveCallView, cx: &mut Context<Oxide>) -> AnyElement {
    let theme = this.theme;
    let current = this.ui.call.layout.current();
    let open = this.popup_open("call.layout");
    let button = div()
        .id("call-layout")
        .w(px(44.))
        .h(px(38.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(10.))
        .cursor_pointer()
        .bg(theme.input)
        .child(icon(current.icon(), px(16.), theme.text))
        .tooltip(widgets::tooltip(tr("ui.call_window.layout"), theme))
        .on_click(cx.listener(|this, _, _, cx| {
            this.toggle_popup("call.layout", cx);
            redraw(this, cx);
        }));
    let menu = open.then(|| {
        let mut items: Vec<AnyElement> = CallLayout::ALL
            .into_iter()
            .map(|option| {
                widgets::menu_item(
                    SharedString::from(format!("layout-{option:?}")),
                    Some(if option == current {
                        Icon::Check
                    } else {
                        option.icon()
                    }),
                    option.title(),
                    &theme,
                )
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.close_popup(cx);
                    let ui = &mut this.ui.call;
                    ui.layout.pick(option, &mut ui.prefs);
                    redraw(this, cx);
                }))
                .into_any_element()
            })
            .collect();
        if let Some(pinned) = this.ui.call.layout.pinned.clone() {
            let name = call
                .participants
                .iter()
                .find(|p| p.id == pinned)
                .map(|p| p.name.clone())
                .unwrap_or_default();
            items.push(
                div()
                    .my_1()
                    .child(widgets::divider(&theme))
                    .into_any_element(),
            );
            items.push(
                widgets::menu_item(
                    "layout-unpin",
                    None,
                    tr("ui.call_window.unpin") + &name,
                    &theme,
                )
                .on_click(cx.listener(|this, _, _, cx| {
                    this.close_popup(cx);
                    this.ui.call.layout.pinned = None;
                    redraw(this, cx);
                }))
                .into_any_element(),
            );
        }
        menu_up(items, &theme, closer(cx))
    });
    // The menu anchors at the button's top edge and opens upward.
    div()
        .flex()
        .flex_col()
        .children(menu)
        .child(button)
        .into_any_element()
}

fn controls(this: &mut Oxide, call: &ActiveCallView, cx: &mut Context<Oxide>) -> AnyElement {
    let theme = this.theme;
    let connected = call.phase == CallPhaseView::Connected;
    let camera_disabled = !connected && !call.video;
    let muted = call.muted;
    let camera_on = call.camera;
    let sharing = call.sharing_screen;
    let speaker = call.speaker;
    let banners_muted = this.notifications.as_ref().is_some_and(|n| n.muted_in_call);
    let mut bar =
        div()
            .flex_none()
            .py(px(12.))
            .bg(theme.surface)
            .flex()
            .items_center()
            .justify_center()
            .gap(px(12.))
            .child(
                toggle(
                    "call-mic",
                    !muted,
                    if muted { Icon::MicOff } else { Icon::Mic },
                    if muted {
                        tr("ui.call_window.unmute")
                    } else {
                        tr("ui.call_window.mute")
                    },
                    &theme,
                )
                .on_click(cx.listener(move |this, _, _, _| {
                    send(this, CallAction::SetMuted { muted: !muted })
                })),
            )
            .child(
                faded(
                    toggle(
                        "call-camera",
                        camera_on,
                        if camera_on {
                            Icon::Video
                        } else {
                            Icon::VideoOff
                        },
                        if camera_on {
                            tr("ui.call_window.turn_camera_off")
                        } else {
                            tr("ui.call_window.turn_camera_on")
                        },
                        &theme,
                    ),
                    camera_disabled,
                )
                .when(!camera_disabled, |b| {
                    b.on_click(cx.listener(move |this, _, _, _| {
                        send(this, CallAction::SetCamera { on: !camera_on })
                    }))
                }),
            );
    let share_call = call.clone();
    bar = bar.child(
        faded(
            toggle(
                "call-share",
                sharing,
                if sharing {
                    Icon::ScreenOff
                } else {
                    Icon::Screen
                },
                if sharing {
                    tr("ui.call_window.stop_sharing")
                } else {
                    tr("ui.call_window.share_screen")
                },
                &theme,
            ),
            !connected,
        )
        .when(connected, |b| {
            b.on_click(cx.listener(move |this, _, _, cx| {
                toggle_share(this, &share_call);
                redraw(this, cx);
            }))
        }),
    );
    if connected {
        bar = bar.child(record_menu(this, call, cx));
    }
    bar = bar.child(
        toggle(
            "call-speaker",
            speaker,
            if speaker {
                Icon::Speaker
            } else {
                Icon::SpeakerOff
            },
            if speaker {
                tr("ui.call_window.mute_speaker")
            } else {
                tr("ui.call_window.unmute_speaker")
            },
            &theme,
        )
        .on_click(
            cx.listener(move |this, _, _, _| send(this, CallAction::SetSpeaker { on: !speaker })),
        ),
    );
    if connected {
        bar = bar.child(layout_menu(this, call, cx));
    }
    if call.is_meeting {
        let show = this.ui.call.show_roster;
        bar = bar.child(
            toggle(
                "call-roster",
                show,
                Icon::People,
                tr("ui.call_window.participants"),
                &theme,
            )
            .on_click(cx.listener(|this, _, _, cx| {
                this.ui.call.show_roster = !this.ui.call.show_roster;
                redraw(this, cx);
            })),
        );
    }
    bar = bar.child(
        toggle(
            "call-banners",
            !banners_muted,
            if banners_muted {
                Icon::BellOff
            } else {
                Icon::Bell
            },
            if banners_muted {
                tr("ui.call_window.show_notifications_again")
            } else {
                tr("ui.call_window.hide_notifications_during_this_call")
            },
            &theme,
        )
        .on_click(cx.listener(move |this, _, _, _| {
            this.send(Action::Notifications {
                action: NotificationsAction::MuteDuringThisCall {
                    muted: !banners_muted,
                },
            });
        })),
    );
    let ending = call.phase == CallPhaseView::Ending;
    let leave = div()
        .id("call-leave")
        .h(px(38.))
        .px(px(16.))
        .flex()
        .items_center()
        .gap(px(6.))
        .rounded(px(10.))
        .bg(theme.danger)
        .text_color(gpui::white())
        .font_weight(FontWeight::SEMIBOLD)
        .child(icon(Icon::PhoneOff, px(15.), gpui::white()))
        .child(if calling(call) {
            tr("ui.call_window.cancel_call")
        } else {
            tr("ui.call_window.leave")
        })
        .map(|d| {
            if ending {
                d.opacity(0.45)
            } else {
                d.cursor_pointer().hover(|s| s.opacity(0.9))
            }
        })
        .when(!ending, |d| {
            d.on_click(cx.listener(|this, _, _, _| send(this, CallAction::Hangup)))
        });
    bar.child(leave).into_any_element()
}

fn end_bar(this: &Oxide, cx: &mut Context<Oxide>) -> AnyElement {
    let theme = this.theme;
    div()
        .flex_none()
        .py(px(14.))
        .bg(theme.surface)
        .flex()
        .justify_center()
        .child(
            widgets::button(
                "call-close",
                tr("ui.call_window.close"),
                ButtonKind::Primary,
                &theme,
            )
            .on_click(cx.listener(|this, _, _, _| send(this, CallAction::Dismiss))),
        )
        .into_any_element()
}

fn roster(this: &Oxide, call: &ActiveCallView) -> AnyElement {
    let theme = this.theme;
    let waiting: Vec<_> = call.participants.iter().filter(|p| p.in_lobby).collect();
    let present: Vec<_> = call.participants.iter().filter(|p| !p.in_lobby).collect();
    let section = |title: String| {
        div()
            .px(px(14.))
            .pt(px(12.))
            .pb(px(4.))
            .text_size(px(11.))
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(theme.muted)
            .child(title)
    };
    let line = || {
        div()
            .px(px(14.))
            .h(px(28.))
            .flex()
            .items_center()
            .gap(px(8.))
    };
    div()
        .id("call-roster-list")
        .w(px(240.))
        .flex_none()
        .h_full()
        .overflow_y_scroll()
        .bg(theme.surface)
        .border_l_1()
        .border_color(theme.border)
        .child(section(trf(
            "ui.call_window.in_the_meeting",
            &[("value", &(present.len() + 1).to_string())],
        )))
        .child(
            line()
                .child(icon(Icon::Person, px(14.), theme.accent))
                .child(tr("ui.call_window.you")),
        )
        .children(present.into_iter().map(|p| {
            line()
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .whitespace_nowrap()
                        .overflow_hidden()
                        .text_ellipsis()
                        .child(p.name.clone()),
                )
                .when_some(role(&p.role), |d, role| {
                    d.child(widgets::muted(role, 11., &theme))
                })
        }))
        .when(!waiting.is_empty(), |d| {
            d.child(section(trf(
                "ui.call_window.lobby",
                &[("waitingCount", &waiting.len().to_string())],
            )))
            .children(
                waiting
                    .into_iter()
                    .map(|p| line().text_color(theme.muted).child(p.name.clone())),
            )
        })
        .into_any_element()
}

/// The callee's avatar with waves going out while their side rings.
fn ringing_avatar(this: &mut Oxide, call: &ActiveCallView) -> AnyElement {
    let theme = this.theme;
    let ringing = call.phase == CallPhaseView::Ringing;
    let avatar = this.avatar(&call.id, &call.title, 96., AvatarShape::Round, None);
    let wave = |offset: f32| {
        div()
            .absolute()
            .size(px(96.))
            .rounded_full()
            .border(px(2.))
            .border_color(theme.accent.opacity(0.5))
            .with_animation(
                SharedString::from(format!("ring-wave-{offset}")),
                Animation::new(Duration::from_millis(1600)).repeat(),
                move |d, delta| {
                    let t = (delta + offset) % 1.;
                    // easeOut, from the avatar's size to 1.9 times it, fading.
                    let eased = 1. - (1. - t) * (1. - t);
                    d.size(px(96. * (1. + 0.9 * eased))).opacity(1. - eased)
                },
            )
    };
    div()
        .size(px(190.))
        .relative()
        .flex()
        .items_center()
        .justify_center()
        .when(ringing, |d| d.child(wave(0.)).child(wave(0.5)))
        .child(avatar)
        .into_any_element()
}

fn stage(
    this: &mut Oxide,
    call: &ActiveCallView,
    window: &mut Window,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    let theme = this.theme;
    if calling(call) {
        let local = (call.video && call.camera)
            .then(|| this.ui.call.media.local_video.clone())
            .flatten();
        let show_local = call.video && call.camera;
        return div()
            .size_full()
            .relative()
            .rounded(px(14.))
            .bg(theme.conversation)
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap(px(18.))
            .child(ringing_avatar(this, call))
            .child(
                div()
                    .text_size(px(22.))
                    .font_weight(FontWeight::SEMIBOLD)
                    .text_center()
                    .child(call.title.clone()),
            )
            .child(widgets::muted(
                if call.phase == CallPhaseView::Ringing {
                    tr("ui.call_window.ringing_waiting_for_an_answer")
                } else {
                    tr("ui.call_window.calling")
                },
                14.,
                &theme,
            ))
            .when(show_local, |d| {
                d.child(
                    div()
                        .absolute()
                        .right(px(12.))
                        .bottom(px(12.))
                        .w(px(200.))
                        .h(px(112.))
                        .shadow_lg()
                        .child(video_tile(local.as_ref(), tr("ui.call_window.you"), &theme)),
                )
            })
            .into_any_element();
    }
    if call.phase == CallPhaseView::Connected {
        return call_stage::stage(this, call, window, cx);
    }
    if call.video && call.phase != CallPhaseView::Ended {
        let local = this.ui.call.media.local_video.clone();
        return video_tile(local.as_ref(), placeholder(call), &theme);
    }
    let shape = if call.is_meeting {
        AvatarShape::Square
    } else {
        AvatarShape::Round
    };
    div()
        .size_full()
        .rounded(px(14.))
        .bg(theme.conversation)
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(14.))
        .child(this.avatar(&call.id, &call.title, 96., shape, None))
        .child(
            div()
                .text_size(px(14.))
                .text_color(theme.muted)
                .child(placeholder(call)),
        )
        .into_any_element()
}

fn in_call_view(
    this: &mut Oxide,
    call: &ActiveCallView,
    error: Option<String>,
    window: &mut Window,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    let theme = this.theme;
    let mut banners = Vec::new();
    if call.phase == CallPhaseView::Lobby {
        banners.push(banner(
            Icon::Clock,
            call.status.clone(),
            theme.accent_soft,
            &theme,
        ));
    }
    if call.is_test && call.phase == CallPhaseView::Connected {
        banners.push(banner(
            Icon::Wave,
            tr("ui.call_window.speak_after_the_beep_the_echo"),
            theme.accent_soft,
            &theme,
        ));
    }
    if let Some(notice) = call
        .notice
        .clone()
        .filter(|_| call.phase != CallPhaseView::Ended)
    {
        banners.push(banner(Icon::Record, notice, theme.error_background, &theme));
    }
    if call.sharing_screen && call.phase == CallPhaseView::Connected {
        banners.push(banner(
            Icon::Screen,
            tr("ui.call_window.you_re_sharing_your_screen"),
            theme.accent_soft,
            &theme,
        ));
    }
    if this.ui.call.media.reconnecting && call.phase == CallPhaseView::Connected {
        banners.push(banner(
            Icon::WifiOff,
            tr("ui.call_window.reconnecting"),
            theme.error_background,
            &theme,
        ));
    }
    banners.extend(recording_banners(this, call, cx));
    if let Some(error) = error {
        banners.push(banner(Icon::Alert, error, theme.error_background, &theme));
    }
    let show_roster =
        this.ui.call.show_roster && call.is_meeting && call.phase != CallPhaseView::Ended;
    let stage = stage(this, call, window, cx);
    let bottom = if call.phase == CallPhaseView::Ended {
        end_bar(this, cx)
    } else {
        controls(this, call, cx)
    };
    div()
        .size_full()
        .flex()
        .flex_col()
        .child(header(call, &theme))
        .children(banners)
        .child(
            div()
                .flex_1()
                .min_h_0()
                .flex()
                .child(div().flex_1().min_w_0().p(px(16.)).child(stage))
                .when(show_roster, |d| d.child(roster(this, call))),
        )
        .child(bottom)
        .into_any_element()
}

// MARK: Debug

/// `OXIDE_SCRIPT` steps: `call:start audio|video` (1:1 with Alice), `call:chat audio|video` (the selected chat),
/// `call:incoming audio|video`, `call:accept`, `call:decline`, `call:test` (Echo Bot), `call:meeting [title]`
/// (pre-join of a meeting link), `call:join`, `call:mute`, `call:camera`, `call:hangup`, `call:dismiss`,
/// `call:front` (raise the call window: macOS stops drawing a covered one), `call:roster`, `call:share` (the share
/// sheet), `call:menu record|layout`, `call:layout gallery|speaker|sharing|focus`, `call:record` (on this Mac),
/// `call:report` (no audio received, for the test report), `call:testcall` (Settings' test call button).
pub fn debug_step(
    this: &mut Oxide,
    step: &str,
    window: &mut Window,
    cx: &mut Context<Oxide>,
) -> bool {
    let _ = window;
    let Some(rest) = step.strip_prefix("call:") else {
        return false;
    };
    let (verb, arg) = rest.split_once(' ').unwrap_or((rest, ""));
    let video = arg == "video";
    let active = this.call.as_ref().and_then(|p| p.active.clone());
    match verb {
        "start" => send(
            this,
            CallAction::Start {
                mri: "8:orgid:alice".into(),
                video,
            },
        ),
        "chat" => send(
            this,
            CallAction::CallChat {
                chat_id: None,
                video,
            },
        ),
        "incoming" => send(this, CallAction::SimulateIncoming { video }),
        "accept" => send(this, CallAction::Accept { video }),
        "decline" => send(this, CallAction::Decline),
        "test" => send(this, CallAction::TestCall { bot_mri: None }),
        "meeting" => send(
            this,
            CallAction::PrepareMeeting {
                url: "https://teams.microsoft.com/l/meetup-join/19%3ameeting_demo%40thread.v2/0"
                    .into(),
                title: if arg.is_empty() {
                    "Point hebdo".into()
                } else {
                    arg.into()
                },
            },
        ),
        "join" => join(this, false),
        "mute" => send(
            this,
            CallAction::SetMuted {
                muted: !active.is_some_and(|c| c.muted),
            },
        ),
        "camera" => send(
            this,
            CallAction::SetCamera {
                on: !active.is_some_and(|c| c.camera),
            },
        ),
        "hangup" => send(this, CallAction::Hangup),
        "front" => {
            if let Some(handle) = this.ui.call.window {
                cx.activate(true);
                let _ = handle.update(cx, |_, window, _| window.activate_window());
            }
        }
        "dismiss" => send(this, CallAction::Dismiss),
        "roster" => this.ui.call.show_roster = !this.ui.call.show_roster,
        "share" => call_share_picker::open(this),
        "menu" => this.popup = Some(format!("call.{arg}").into()),
        "layout" => {
            let layout = match arg {
                "speaker" => CallLayout::Speaker,
                "sharing" => CallLayout::Sharing,
                "focus" => CallLayout::Focus,
                _ => CallLayout::Gallery,
            };
            let ui = &mut this.ui.call;
            ui.layout.pick(layout, &mut ui.prefs);
        }
        "record" => {
            if let Some(call) = active {
                start_local_recording(this, &call);
            }
        }
        "report" => {
            let media = &mut this.ui.call.media;
            media.diagnostics.connected = true;
            media.diagnostics.audio_packets_received = 0;
        }
        "testcall" => call_devices::start_test_call(this),
        _ => return false,
    }
    redraw(this, cx);
    cx.notify();
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elapsed_shows_hours_only_past_an_hour() {
        let at = |s: u64| SystemTime::UNIX_EPOCH + Duration::from_secs(s);
        assert_eq!(elapsed(1_000_000, at(1_000 + 65)), "01:05");
        assert_eq!(elapsed(1_000_000, at(1_000 + 3_725)), "1:02:05");
        assert_eq!(elapsed(1_000_000, at(900)), "00:00");
    }

    #[test]
    fn roles_have_labels() {
        assert!(role("organizer").is_some());
        assert!(role("").is_none());
    }
}

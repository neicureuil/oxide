//! Microphone, speaker and camera choice, device test and test call: the Settings › Audio and video page
//! (`DeviceSettingsSection.swift`), and the device pickers the pre-join screen shares with it.
use crate::icons::{Icon, icon};
use crate::media_devices::{MediaKind, permissions};
use crate::media_engine::Window as Target;
use crate::media_tester::{self, CameraPreview, Owner};
use crate::oxide::{Oxide, tr, trf};
use crate::widgets::{self, ButtonKind};
use gpui::{
    AnyElement, Context, Corner, FontWeight, PathPromptOptions, Window, div, prelude::*, px,
};
use std::time::{Duration, Instant};
use teams_core::{Action, CallAction};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceKind {
    Microphone,
    Speaker,
    Camera,
}

/// Where titles start: padding, icon slot, spacing.
const INSET: f32 = 16. + 18. + 10.;

/// Loads the device lists the first time they are needed, and follows CoreAudio's changes from then on.
pub fn ensure_devices(this: &mut Oxide) {
    let ui = &mut this.ui.call;
    if ui.devices_loaded {
        return;
    }
    ui.devices_loaded = true;
    ui.devices.refresh();
    #[cfg(target_os = "macos")]
    {
        let tx = ui.media.tx.clone();
        crate::media_devices::core_audio::observe(move || {
            let _ = tx.unbounded_send(crate::media_engine::MediaMsg::Devices);
        });
    }
}

fn chosen(this: &Oxide, kind: DeviceKind) -> String {
    match kind {
        DeviceKind::Microphone => this.prefs.microphone.clone(),
        DeviceKind::Speaker => this.prefs.speaker.clone(),
        DeviceKind::Camera => this.prefs.camera.clone(),
    }
}

/// Stores the device (by UID / unique id; empty for the system default) and restarts what uses it.
fn choose(this: &mut Oxide, kind: DeviceKind, id: String, cx: &mut Context<Oxide>) {
    match kind {
        DeviceKind::Microphone => this.prefs.microphone = id,
        DeviceKind::Speaker => this.prefs.speaker = id,
        DeviceKind::Camera => this.prefs.camera = id,
    }
    this.prefs.save();
    match kind {
        DeviceKind::Microphone => {
            for owner in [Owner::Settings, Owner::Prejoin] {
                let running = match owner {
                    Owner::Settings => this.ui.call.settings_mic.running,
                    Owner::Prejoin => this.ui.call.prejoin_mic.running,
                };
                if running {
                    media_tester::stop_mic(this, owner);
                    media_tester::start_mic(this, owner, cx);
                }
            }
        }
        DeviceKind::Camera => {
            let device = crate::media_devices::resolved_camera(&this.prefs);
            for preview in [
                &mut this.ui.call.settings_preview,
                &mut this.ui.call.prejoin_preview,
            ]
            .into_iter()
            .flatten()
            {
                preview.show(device.clone());
            }
        }
        DeviceKind::Speaker => {}
    }
}

/// A device menu: "System default", each connected device, and a disconnected choice kept as such.
/// `settings`: the Settings page's labels (with the default device's name).
pub fn picker(
    this: &mut Oxide,
    kind: DeviceKind,
    settings: bool,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    ensure_devices(this);
    let theme = this.theme;
    let devices = &this.ui.call.devices;
    let options: Vec<(String, String)> = match kind {
        DeviceKind::Microphone => devices
            .microphones
            .iter()
            .map(|d| (d.uid.clone(), d.name.clone()))
            .collect(),
        DeviceKind::Speaker => devices
            .speakers
            .iter()
            .map(|d| (d.uid.clone(), d.name.clone()))
            .collect(),
        DeviceKind::Camera => devices
            .cameras
            .iter()
            .map(|d| (d.unique_id.clone(), d.name.clone()))
            .collect(),
    };
    let system_name = match kind {
        DeviceKind::Microphone => devices.default_microphone.clone(),
        DeviceKind::Speaker => devices.default_speaker.clone(),
        DeviceKind::Camera => devices.default_camera.clone(),
    };
    let system = if settings {
        system_name.map_or_else(
            || tr("ui.device_settings_section.system_default_2"),
            |name| {
                trf(
                    "ui.device_settings_section.system_default",
                    &[("device", &name)],
                )
            },
        )
    } else {
        tr("ui.call_window.system_default")
    };
    let missing = tr(if settings {
        "ui.device_settings_section.device_not_connected"
    } else {
        "ui.call_window.device_not_connected"
    });
    let selection = chosen(this, kind);
    let label = if selection.is_empty() {
        system.clone()
    } else {
        options
            .iter()
            .find(|(id, _)| *id == selection)
            .map_or(missing.clone(), |(_, name)| name.clone())
    };
    let mut entries: Vec<(String, String)> = vec![(String::new(), system)];
    entries.extend(options.iter().cloned());
    if !selection.is_empty() && !options.iter().any(|(id, _)| *id == selection) {
        entries.push((selection.clone(), missing));
    }
    let key = format!("devices.{kind:?}.{settings}");
    let open = this.popup_open(&key);
    let toggle_key = key.clone();
    let trigger = div()
        .id(gpui::SharedString::from(format!("{key}.button")))
        .h(px(28.))
        .px(px(10.))
        .min_w(px(160.))
        .max_w(px(300.))
        .flex()
        .items_center()
        .gap(px(6.))
        .rounded(px(7.))
        .border_1()
        .border_color(theme.border_strong)
        .bg(theme.surface)
        .text_size(px(13.))
        .cursor_pointer()
        .hover(move |s| s.bg(theme.input))
        .on_click(cx.listener(move |this, _, _, cx| {
            if kind == DeviceKind::Camera {
                this.ui.call.devices.refresh();
            }
            this.toggle_popup(toggle_key.clone(), cx);
            crate::call_window::redraw(this, cx);
        }))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis()
                .child(label),
        )
        .child(icon(Icon::ChevronDown, px(12.), theme.muted));
    let menu = open.then(|| {
        let items: Vec<AnyElement> = entries
            .into_iter()
            .enumerate()
            .map(|(i, (id, name))| {
                let selected = id == selection;
                widgets::menu_item(
                    gpui::SharedString::from(format!("{key}.{i}")),
                    selected.then_some(Icon::Check),
                    name,
                    &theme,
                )
                .when(!selected, |d| d.pl(px(31.)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.close_popup(cx);
                    choose(this, kind, id.clone(), cx);
                    crate::call_window::redraw(this, cx);
                }))
                .into_any_element()
            })
            .collect();
        widgets::popover(
            Corner::TopLeft,
            div().max_h(px(320.)).children(items),
            &theme,
            {
                let entity = cx.entity().downgrade();
                move |_, cx| {
                    let _ = entity.update(cx, |this, cx| {
                        this.close_popup(cx);
                        crate::call_window::redraw(this, cx);
                    });
                }
            },
        )
    });
    div()
        .relative()
        .child(trigger)
        .children(menu)
        .into_any_element()
}

/// A level meter, red near full scale.
pub fn level_meter(
    level: f32,
    active: bool,
    width: f32,
    theme: &crate::theme::Theme,
) -> AnyElement {
    let level = if active { level.clamp(0., 1.) } else { 0. };
    div()
        .w(px(width))
        .h(px(6.))
        .rounded_full()
        .bg(theme.input)
        .child(
            div()
                .h_full()
                .w(px(width * level))
                .rounded_full()
                .bg(if level > 0.85 {
                    theme.danger
                } else {
                    theme.accent
                }),
        )
        .into_any_element()
}

/// One line of a device card: icon and title (and an optional detail line) on the left, the control pushed to the
/// right edge. Lines without an icon keep its slot, so titles align.
fn row(
    title: String,
    glyph: Option<Icon>,
    detail: Option<(String, bool)>,
    trailing: impl IntoElement,
    theme: &crate::theme::Theme,
) -> AnyElement {
    div()
        .min_h(px(52.))
        .px(px(16.))
        .flex()
        .items_center()
        .gap(px(10.))
        .child(
            div()
                .w(px(18.))
                .flex_none()
                .when_some(glyph, |d, g| d.child(icon(g, px(16.), theme.muted))),
        )
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(3.))
                .flex_1()
                .min_w_0()
                .child(
                    div()
                        .text_size(px(13.))
                        .when(glyph.is_some(), |d| d.font_weight(FontWeight::SEMIBOLD))
                        .child(title),
                )
                .when_some(detail, |d, (text, error)| {
                    d.child(
                        div()
                            .text_size(px(12.))
                            .text_color(if error { theme.danger } else { theme.muted })
                            .child(text),
                    )
                }),
        )
        .child(
            div()
                .flex_none()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(trailing),
        )
        .into_any_element()
}

fn inset_divider(theme: &crate::theme::Theme) -> AnyElement {
    div()
        .pl(px(INSET))
        .child(widgets::divider(theme))
        .into_any_element()
}

fn problem(
    text: String,
    kind: MediaKind,
    theme: &crate::theme::Theme,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    div()
        .pl(px(INSET))
        .pr(px(16.))
        .pb(px(16.))
        .flex()
        .items_center()
        .gap(px(8.))
        .child(
            div()
                .text_size(px(12.))
                .text_color(theme.danger)
                .child(text),
        )
        .child(
            div()
                .id(gpui::SharedString::from(format!("open-settings-{kind:?}")))
                .text_size(px(12.))
                .text_color(theme.accent_text)
                .cursor_pointer()
                .on_click(
                    cx.listener(move |_, _, _, cx| cx.open_url(permissions::settings_url(kind))),
                )
                .child(tr("ui.device_settings_section.open_settings")),
        )
        .into_any_element()
}

/// Stops the microphone test and the camera preview once the page is no longer drawn (SwiftUI's `onDisappear`):
/// both redraw the page many times a second while it shows.
fn watch(this: &mut Oxide, cx: &mut Context<Oxide>) {
    let ui = &mut this.ui.call;
    ui.settings_seen = Some(Instant::now());
    if ui.settings_watch.is_some() || (!ui.settings_mic.running && ui.settings_preview.is_none()) {
        return;
    }
    ui.settings_watch = Some(cx.spawn(async move |this, cx| {
        loop {
            cx.background_executor()
                .timer(Duration::from_millis(500))
                .await;
            let keep = this.update(cx, |this, _| {
                let ui = &mut this.ui.call;
                let active = ui.settings_mic.running || ui.settings_preview.is_some();
                let gone = ui
                    .settings_seen
                    .is_none_or(|seen| seen.elapsed() > Duration::from_secs(1));
                if active && gone {
                    media_tester::stop_mic(this, Owner::Settings);
                    this.ui.call.settings_preview = None;
                }
                let ui = &mut this.ui.call;
                let keep = ui.settings_mic.running || ui.settings_preview.is_some();
                if !keep {
                    ui.settings_watch = None;
                }
                keep
            });
            if !matches!(keep, Ok(true)) {
                return;
            }
        }
    }));
}

/// The body of Settings › Audio and video, drawn by the settings screen.
pub fn device_settings(
    this: &mut Oxide,
    window: &mut Window,
    cx: &mut Context<Oxide>,
) -> AnyElement {
    let _ = window;
    ensure_devices(this);
    watch(this, cx);
    let theme = this.theme;
    let card = || widgets::card(&theme).flex().flex_col().overflow_hidden();

    // Microphone
    let mic = &this.ui.call.settings_mic;
    let (level, running, recording, playing_back, mic_error) = (
        mic.level,
        mic.running,
        mic.recording,
        mic.playing_back,
        mic.error.clone(),
    );
    let mic_picker = picker(this, DeviceKind::Microphone, true, cx);
    let echo_busy = recording || playing_back;
    let microphone = card()
        .child(row(
            tr("ui.device_settings_section.microphone"),
            Some(Icon::Mic),
            None,
            mic_picker,
            &theme,
        ))
        .child(inset_divider(&theme))
        .child(row(
            tr("ui.device_settings_section.input_level"),
            None,
            None,
            div()
                .flex()
                .items_center()
                .gap(px(8.))
                .child(
                    div()
                        .pr(px(4.))
                        .child(level_meter(level, running, 140., &theme)),
                )
                .child(
                    widgets::button(
                        "mic-test",
                        if running {
                            tr("ui.device_settings_section.stop")
                        } else {
                            tr("ui.device_settings_section.test_mic")
                        },
                        ButtonKind::Outline,
                        &theme,
                    )
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if running {
                            media_tester::stop_mic(this, Owner::Settings);
                        } else {
                            media_tester::start_mic(this, Owner::Settings, cx);
                        }
                        cx.notify();
                    })),
                )
                .child(
                    widgets::disabled(
                        widgets::button(
                            "mic-echo",
                            if recording {
                                tr("ui.device_settings_section.recording")
                            } else if playing_back {
                                tr("ui.device_settings_section.playing")
                            } else {
                                tr("ui.device_settings_section.hear_yourself_5_s")
                            },
                            ButtonKind::Outline,
                            &theme,
                        ),
                        echo_busy,
                    )
                    .tooltip(widgets::tooltip(
                        tr("ui.device_settings_section.records_5_seconds_then_plays_them"),
                        theme,
                    ))
                    .when(!echo_busy, |b| {
                        b.on_click(cx.listener(|this, _, _, cx| {
                            media_tester::echo(this, Owner::Settings, cx);
                            cx.notify();
                        }))
                    }),
                ),
            &theme,
        ))
        .when_some(mic_error, |d, error| {
            d.child(problem(error, MediaKind::Audio, &theme, cx))
        });

    // Speaker
    let speaker_picker = picker(this, DeviceKind::Speaker, true, cx);
    let (playing, speaker_error) = (
        this.ui.call.speaker_test.playing,
        this.ui.call.speaker_test.error.clone(),
    );
    let speaker = card()
        .child(row(
            tr("ui.device_settings_section.speaker"),
            Some(Icon::Speaker),
            None,
            speaker_picker,
            &theme,
        ))
        .child(inset_divider(&theme))
        .child(row(
            tr("ui.device_settings_section.test_sound"),
            None,
            speaker_error.map(|e| (e, true)),
            widgets::disabled(
                widgets::button(
                    "speaker-test",
                    if playing {
                        tr("ui.device_settings_section.playing")
                    } else {
                        tr("ui.device_settings_section.test_speaker")
                    },
                    ButtonKind::Outline,
                    &theme,
                ),
                playing,
            )
            .when(!playing, |b| {
                b.on_click(cx.listener(|this, _, _, cx| {
                    media_tester::play_chime(this);
                    cx.notify();
                }))
            }),
            &theme,
        ));

    // Camera
    let camera_picker = picker(this, DeviceKind::Camera, true, cx);
    let previewing = this.ui.call.settings_preview.is_some();
    let preview = this
        .ui
        .call
        .settings_preview
        .as_ref()
        .map(|p| crate::media_video::view(&p.slot, false));
    let denied = this.ui.call.settings_camera_denied;
    let camera = card()
        .child(row(
            tr("ui.device_settings_section.camera"),
            Some(Icon::Video),
            None,
            camera_picker,
            &theme,
        ))
        .child(
            div().px(px(16.)).pb(px(16.)).child(
                div()
                    .relative()
                    .h(px(220.))
                    .rounded(px(10.))
                    .overflow_hidden()
                    .bg(theme.input)
                    .flex()
                    .items_center()
                    .justify_center()
                    .map(|d| match preview {
                        Some(video) => d.child(video),
                        None => d.child(icon(Icon::VideoOff, px(22.), theme.subtle)),
                    })
                    .child(
                        div().absolute().right(px(10.)).bottom(px(10.)).child(
                            widgets::button(
                                "camera-preview",
                                if previewing {
                                    tr("ui.device_settings_section.stop_preview")
                                } else {
                                    tr("ui.device_settings_section.preview")
                                },
                                ButtonKind::Outline,
                                &theme,
                            )
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    if previewing {
                                        this.ui.call.settings_preview = None;
                                        cx.notify();
                                    } else {
                                        start_preview(this, cx);
                                    }
                                },
                            )),
                        ),
                    ),
            ),
        )
        .when(denied, |d| {
            d.child(problem(
                tr("ui.device_settings_section.camera_access_is_denied_allow_oxide"),
                MediaKind::Video,
                &theme,
                cx,
            ))
        });

    // Test call
    let realtime = this
        .calls
        .as_ref()
        .map(|c| c.realtime.clone())
        .unwrap_or_default();
    let ready = realtime == tr("core.calls.ready_to_receive_calls");
    let busy = this.in_call();
    let hint = if !ready {
        if realtime.is_empty() {
            tr("ui.device_settings_section.connecting_to_the_calling_service")
        } else {
            realtime
        }
    } else if busy {
        tr("ui.device_settings_section.a_call_is_already_in_progress")
    } else {
        tr("ui.device_settings_section.calls_the_teams_echo_bot_it")
    };
    let test_call = card().py(px(12.)).child(row(
        tr("ui.device_settings_section.test_call"),
        Some(Icon::Phone),
        Some((hint, false)),
        widgets::disabled(
            widgets::button(
                "test-call",
                tr("ui.device_settings_section.make_a_test_call"),
                ButtonKind::Outline,
                &theme,
            ),
            !ready || busy,
        )
        .when(ready && !busy, |b| {
            b.on_click(cx.listener(|this, _, _, _| start_test_call(this)))
        }),
        &theme,
    ));

    // Local recordings
    let folder = &this.ui.call.prefs.recordings_folder;
    let custom = !folder.is_empty();
    let shown = crate::media_recorder::folder(folder)
        .to_string_lossy()
        .into_owned();
    let home = std::env::var("HOME").unwrap_or_default();
    let shown = if !home.is_empty() {
        shown.replacen(&home, "~", 1)
    } else {
        shown
    };
    let recordings = card().child(row(
        tr("ui.device_settings_section.local_recordings"),
        Some(Icon::Record),
        Some((shown, false)),
        div()
            .flex()
            .gap(px(8.))
            .when(custom, |d| {
                d.child(
                    widgets::button(
                        "rec-reset",
                        tr("ui.device_settings_section.reset_folder"),
                        ButtonKind::Outline,
                        &theme,
                    )
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.ui.call.prefs.recordings_folder.clear();
                        this.ui.call.prefs.save();
                        cx.notify();
                    })),
                )
            })
            .child(
                widgets::button(
                    "rec-change",
                    tr("ui.device_settings_section.change_folder"),
                    ButtonKind::Outline,
                    &theme,
                )
                .on_click(cx.listener(|_, _, _, cx| choose_recordings_folder(cx))),
            ),
        &theme,
    ));

    div()
        .flex()
        .flex_col()
        .gap(px(14.))
        .child(microphone)
        .child(speaker)
        .child(camera)
        .child(test_call)
        .child(recordings)
        .child(div().px(px(4.)).child(widgets::muted(
            tr("ui.device_settings_section.default_devices_detail"),
            12.,
            &theme,
        )))
        .into_any_element()
}

/// Settings › Audio et vidéo › Appel de test: a real call with the Echo Bot, then a report.
pub fn start_test_call(this: &mut Oxide) {
    media_tester::stop_mic(this, Owner::Settings);
    this.ui.call.settings_preview = None;
    let bot_mri = this.ui.call.prefs.bot_override();
    this.send(Action::Call {
        action: CallAction::TestCall { bot_mri },
    });
}

fn start_preview(_this: &mut Oxide, cx: &mut Context<Oxide>) {
    cx.spawn(async move |this, cx| {
        let granted = permissions::request(MediaKind::Video).await;
        let _ = this.update(cx, |this, cx| {
            this.ui.call.settings_camera_denied = !granted;
            if granted {
                let tx = this.ui.call.media.tx.clone();
                if let Some(mut preview) = CameraPreview::new(Target::Main, tx) {
                    preview.show(crate::media_devices::resolved_camera(&this.prefs));
                    this.ui.call.settings_preview = Some(preview);
                }
            }
            cx.notify();
        });
    })
    .detach();
}

fn choose_recordings_folder(cx: &mut Context<Oxide>) {
    let paths = cx.prompt_for_paths(PathPromptOptions {
        files: false,
        directories: true,
        multiple: false,
        prompt: Some(tr("ui.device_settings_section.choose_folder").into()),
    });
    cx.spawn(async move |this, cx| {
        let Ok(Ok(Some(paths))) = paths.await else {
            return;
        };
        let Some(path) = paths.into_iter().next() else {
            return;
        };
        let _ = this.update(cx, |this, cx| {
            this.ui.call.prefs.recordings_folder = path.to_string_lossy().into_owned();
            this.ui.call.prefs.save();
            cx.notify();
        });
    })
    .detach();
}

//! Oxide drawn with GPUI: the same headless `teams-core` as the macOS app, called directly instead of through UniFFI.
// Screens are being ported one by one: shared helpers are not all used yet.
#![allow(dead_code)]
// No console window behind the app on Windows (debug builds keep it, for logs).
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
mod activity;
mod avatars;
mod calendar;
mod calendar_dates;
mod calendar_details;
mod calendar_editor;
mod calendar_model;
mod calendar_parts;
mod call_alert;
mod call_devices;
mod call_panel;
mod call_prefs;
mod call_report;
mod call_share_picker;
mod call_stage;
mod call_window;
mod calls_screen;
mod chat_admin;
mod chats_parts;
#[cfg(windows)]
mod clipboard_bitmap;
mod composer;
mod composer_emoji;
mod composer_extensions;
mod composer_fun;
mod composer_mention;
mod composer_spelling;
mod composer_thumbs;
mod composer_toolbar;
mod conversation;
mod feed;
mod feed_blocks;
mod feed_message;
mod feed_react;
mod feed_select;
mod files;
mod files_parts;
mod icons;
mod image_viewer;
mod input;
#[cfg(windows)]
mod login_native;
mod login_window;
mod media_devices;
mod media_engine;
mod media_recorder;
// AVFoundation's microphone test and camera preview; a stand-in elsewhere.
#[cfg_attr(not(target_os = "macos"), path = "media_tester_other.rs")]
mod media_tester;
mod media_video;
mod members;
mod motion;
mod new_chat;
#[cfg(target_os = "macos")]
mod objc_util;
mod oxide;
mod paths;
#[cfg_attr(not(target_os = "macos"), path = "platform_other.rs")]
mod platform;
#[cfg(target_os = "macos")]
mod platform_memory;
#[cfg(target_os = "macos")]
mod platform_app;
#[cfg(target_os = "macos")]
mod platform_media;
#[cfg(target_os = "macos")]
mod platform_network;
#[cfg(target_os = "macos")]
mod platform_notifications;
mod prefs;
mod screens;
mod search;
mod settings;
mod settings_controls;
mod settings_debug;
mod settings_memory;
mod settings_notifications;
mod settings_out_of_office;
mod settings_status;
mod settings_time;
mod settings_updater;
mod shell;
mod sidebar;
mod signin;
mod teams;
mod teams_admin;
mod theme;
mod widgets;

use gpui::{
    App, AppContext, Application, Bounds, KeyBinding, Menu, MenuItem, TitlebarOptions, WindowBounds,
    WindowOptions, actions, point, px, size,
};
use std::borrow::Cow;

actions!(oxide, [Quit, SignOut]);

fn main() {
    let demo = teams_core::demo_available()
        && (std::env::args().any(|a| a == "--demo")
            || std::env::var("TEAMS_DEMO").is_ok_and(|v| v == "1"));
    // GPUI keeps each picture twice, in memory and as a GPU texture: half the core's preview budget
    // keeps the total where the SwiftUI app's single copy is.
    teams_core::set_image_budget(32 << 20, 8 << 20);
    if let Some(caches) = paths::caches() {
        teams_core::set_cache_directory(caches.to_string_lossy().into_owned());
    }

    Application::new().with_assets(icons::Assets).run(move |cx: &mut App| {
        cx.text_system()
            .add_fonts(vec![
                Cow::Borrowed(include_bytes!("../../macos/Resources/Fonts/Figtree-Regular.ttf")),
                Cow::Borrowed(include_bytes!("../../macos/Resources/Fonts/Figtree-Medium.ttf")),
                Cow::Borrowed(include_bytes!("../../macos/Resources/Fonts/Figtree-SemiBold.ttf")),
                Cow::Borrowed(include_bytes!("../../macos/Resources/Fonts/Figtree-Italic.ttf")),
            ])
            .expect("bundled fonts");

        cx.bind_keys([KeyBinding::new("cmd-q", Quit, None)]);
        input::bind_keys(cx);
        shell::bind_keys(cx);
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.set_menus(vec![Menu {
            name: "Oxide".into(),
            items: vec![
                MenuItem::action(oxide::text("Se déconnecter", "Sign out"), SignOut),
                MenuItem::separator(),
                MenuItem::action(oxide::text("Quitter Oxide", "Quit Oxide"), Quit),
            ],
        }]);
        // The call window closes on its own: quit only once the main window is gone.
        cx.on_window_closed(|cx| {
            if cx.windows().iter().all(|w| w.downcast::<oxide::Oxide>().is_none()) {
                cx.quit();
            }
        })
        .detach();

        let bounds = Bounds::centered(None, size(px(1180.), px(780.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(960.), px(600.))),
                titlebar: Some(TitlebarOptions {
                    title: Some("Oxide".into()),
                    // macOS draws the traffic lights over our top bar. Windows keeps its own title bar: without it
                    // GPUI leaves the window with no caption to drag and no buttons.
                    appears_transparent: cfg!(target_os = "macos"),
                    traffic_light_position: Some(point(px(14.), px(14.))),
                }),
                ..Default::default()
            },
            |window, cx| cx.new(|cx| oxide::Oxide::new(demo, window, cx)),
        )
        .expect("main window");
        cx.activate(true);
    });
}

//! System services outside the window on Windows and Linux. Windows shows banners (`notifications_windows.rs`);
//! the other macOS ones (`platform.rs`: Dock badge, Quick Look, voicemail playback, network path) are not ported
//! yet: these keep the app running without them, and open downloaded files with the system's default app.
use crate::oxide::Oxide;
use gpui::{Context, Window};
use teams_core::NotificationSound;

pub struct Banner {
    pub id: String,
    pub conversation_id: String,
    pub reply_chain_id: Option<String>,
    pub title: String,
    pub subtitle: Option<String>,
    pub body: String,
    pub sound: Option<NotificationSound>,
    pub urgent: bool,
    pub can_reply: bool,
}

/// Windows: banners, whose clicks come back here. Linux: nothing yet, the app counts itself online.
pub fn start(_window: &mut Window, cx: &mut Context<Oxide>) {
    #[cfg(windows)]
    {
        use futures::StreamExt;
        let (clicks, mut received) = futures::channel::mpsc::unbounded();
        crate::notifications_windows::start(clicks);
        cx.spawn(async move |this, cx| {
            while let Some(click) = received.next().await {
                if this
                    .update(cx, |this, cx| crate::notifications_windows::handle(this, click, cx))
                    .is_err()
                {
                    return;
                }
            }
        })
        .detach();
    }
    #[cfg(not(windows))]
    let _ = cx;
}

pub fn dock_badge(_unread: Option<(u32, u32)>) {}

pub fn notify(banner: Banner) {
    #[cfg(windows)]
    crate::notifications_windows::notify(&banner);
    #[cfg(not(windows))]
    let _ = banner;
}

pub fn withdraw(conversation_id: &str) {
    #[cfg(windows)]
    crate::notifications_windows::withdraw(conversation_id);
    #[cfg(not(windows))]
    let _ = conversation_id;
}

pub fn notify_call(id: &str, title: &str, video: bool) {
    #[cfg(windows)]
    crate::notifications_windows::notify_call(id, title, video);
    #[cfg(not(windows))]
    let _ = (id, title, video);
}

pub fn withdraw_call(id: &str) {
    #[cfg(windows)]
    crate::notifications_windows::withdraw_call(id);
    #[cfg(not(windows))]
    let _ = id;
}

pub fn refresh_permission() {}

pub fn request_permission() {}

/// Opens the notification settings: none of the app's own on this platform.
pub fn open_notification_settings() {}

/// Our window is in front (Linux, without banners, counts itself in front).
pub fn is_active() -> bool {
    #[cfg(windows)]
    return crate::notifications_windows::is_active();
    #[cfg(not(windows))]
    true
}

pub fn request_attention(_critical: bool) -> Option<isize> {
    None
}

pub fn cancel_attention(_id: isize) {}

/// Opens a downloaded file in its default app, as Quick Look shows it on macOS.
pub fn quick_look(path: &str) {
    open_with_default_app(path);
}

pub fn downloaded(_path: &str) {}

pub fn play_voicemail(_id: &str, _data: Vec<u8>) {}

pub fn stop_voicemail() {}

pub fn voicemail_playing() -> Option<String> {
    None
}

fn open_with_default_app(path: &str) {
    #[cfg(windows)]
    let opened = std::process::Command::new("explorer.exe").arg(path).spawn();
    #[cfg(not(windows))]
    let opened = std::process::Command::new("xdg-open").arg(path).spawn();
    if let Err(error) = opened {
        eprintln!("oxide: could not open {path}: {error}");
    }
}

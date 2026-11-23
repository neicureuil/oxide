//! System services outside the window on Windows and Linux. The macOS ones (`platform.rs`: Dock badge,
//! banners, Quick Look, voicemail playback, network path) are not ported yet: these keep the app running
//! without them, and open downloaded files with the system's default app.
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

/// Nothing to wire yet: the app counts itself online and shows no banners.
pub fn start(_window: &mut Window, _cx: &mut Context<Oxide>) {}

pub fn dock_badge(_unread: Option<(u32, u32)>) {}

pub fn notify(_banner: Banner) {}

pub fn withdraw(_conversation_id: &str) {}

pub fn notify_call(_id: &str, _title: &str, _video: bool) {}

pub fn withdraw_call(_id: &str) {}

pub fn refresh_permission() {}

pub fn request_permission() {}

/// Opens the notification settings: none of the app's own on this platform.
pub fn open_notification_settings() {}

/// Banners aren't shown off macOS: the app counts as in front.
pub fn is_active() -> bool {
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

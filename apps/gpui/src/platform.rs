//! macOS services outside the window: Dock badge, notification banners, Quick Look, voicemail playback,
//! network path and quitting mid-call. AppKit and Network.framework call back on their own threads:
//! everything they report goes through one channel to `start`'s task, on the UI thread.
use crate::oxide::Oxide;
use crate::platform_app;
use futures::StreamExt;
use futures::channel::mpsc::{UnboundedSender, unbounded};
use gpui::{Context, Task, Window};
use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSRequestUserAttentionType};
use objc2_foundation::{NSDistributedNotificationCenter, NSString};
use std::cell::RefCell;
use std::sync::OnceLock;
use std::time::Duration;
use teams_core::{
    Action, CallAction, NotificationPermission, NotificationSound, NotificationsAction,
};

// For the screens that show them (calls, settings).
#[allow(unused_imports)]
pub use crate::platform_media::{
    SystemSound, play_system_sound, set_speaker, stop_voicemail, voicemail_error, voicemail_playing,
};
#[allow(unused_imports)]
pub use crate::platform_notifications::{
    open_notification_settings, permission, refresh_permission, request_permission,
};

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

/// What the system reported, handed to the UI thread.
pub(crate) enum Event {
    /// A usable network path exists (or not), on every path update.
    Online(bool),
    /// Interfaces changed while online, or the Mac woke up: reconnect, debounced.
    NetworkChanged,
    Permission(NotificationPermission),
    Response(Response),
    VoicemailFinished(u64),
    /// AppKit asked to quit: hang up first when in a call.
    Terminate,
}

/// A click or an action on a banner.
pub(crate) struct Response {
    pub request_id: String,
    pub call: bool,
    pub action: ResponseAction,
    pub conversation_id: String,
    pub reply_chain_id: Option<String>,
}

pub(crate) enum ResponseAction {
    Open,
    Dismiss,
    Reply(String),
    MarkRead,
    Accept,
    Decline,
}

static EVENTS: OnceLock<UnboundedSender<Event>> = OnceLock::new();

/// From any thread; false once the window's task is gone.
pub(crate) fn post(event: Event) -> bool {
    EVENTS
        .get()
        .is_some_and(|events| events.unbounded_send(event).is_ok())
}

/// Wires the system services to the store; once, from `Oxide::new` (as `Store.init` and `AppDelegate`).
pub fn start(window: &mut Window, cx: &mut Context<Oxide>) {
    let _ = window;
    let (sender, mut events) = unbounded();
    if EVENTS.set(sender).is_err() {
        return;
    }
    // Before any banner response is delivered, including the one that launched the app.
    crate::platform_notifications::configure();
    crate::platform_notifications::request_permission();
    crate::platform_network::start();
    platform_app::install();
    dock_icon_when_unbundled();
    // Read by AppKit's quit request, which cannot wait for this window's task.
    cx.observe_self(|this, _| platform_app::set_in_call(this.in_call()))
        .detach();

    cx.spawn(async move |this, cx| {
        let mut reconnect: Option<Task<()>> = None;
        while let Some(event) = events.next().await {
            match event {
                Event::NetworkChanged => {
                    // A burst of path changes (Wi-Fi to Ethernet, wake) gives one reconnect.
                    let Ok(core) = this.update(cx, |this, _| this.core.clone()) else {
                        return;
                    };
                    let timer = cx.background_executor().timer(Duration::from_secs(2));
                    reconnect = Some(cx.background_executor().spawn(async move {
                        timer.await;
                        core.send(Action::Call {
                            action: CallAction::NetworkChanged,
                        });
                    }));
                }
                Event::Terminate => {
                    let in_call = this.update(cx, |this, _| {
                        let in_call = this.in_call();
                        if in_call {
                            this.send(Action::Call {
                                action: CallAction::Hangup,
                            });
                        }
                        in_call
                    });
                    // Others would see a frozen participant otherwise; gives up after 3 s.
                    if in_call.unwrap_or(false) {
                        for _ in 0..30 {
                            cx.background_executor()
                                .timer(Duration::from_millis(100))
                                .await;
                            if !this.update(cx, |this, _| this.in_call()).unwrap_or(false) {
                                break;
                            }
                        }
                    }
                    platform_app::quit_now(cx);
                }
                event => {
                    if this.update(cx, |this, cx| handle(this, event, cx)).is_err() {
                        return;
                    }
                }
            }
        }
        drop(reconnect);
    })
    .detach();
}

/// Run outside a bundle (`cargo run`), macOS shows a generic executable in the Dock: give it the
/// app icon from the repository. The bundle gets it from Info.plist.
fn dock_icon_when_unbundled() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let bundled = objc2_foundation::NSBundle::mainBundle()
        .bundlePath()
        .to_string()
        .ends_with(".app");
    if bundled {
        return;
    }
    let path = concat!(env!("CARGO_MANIFEST_DIR"), "/../macos/Resources/AppIcon.icns");
    let Ok(bytes) = std::fs::read(path) else {
        return;
    };
    let data = objc2_foundation::NSData::with_bytes(&bytes);
    let Some(image) = objc2_app_kit::NSImage::initWithData(<objc2_app_kit::NSImage as objc2::AnyThread>::alloc(), &data) else {
        return;
    };
    unsafe { NSApplication::sharedApplication(mtm).setApplicationIconImage(Some(&image)) };
}

fn handle(this: &mut Oxide, event: Event, cx: &mut Context<Oxide>) {
    match event {
        Event::Online(online) => {
            if this.online == online {
                return;
            }
            this.online = online;
            platform_app::announce(&crate::oxide::tr(if online {
                "ui.store.back_online"
            } else {
                "ui.store.offline"
            }));
            // Back online: fetch what was missed instead of waiting for the next refresh.
            if online && this.shell.as_ref().is_some_and(|s| s.signed_in) {
                this.send(Action::Refresh);
            }
        }
        Event::Permission(permission) => this.send(Action::Notifications {
            action: NotificationsAction::PermissionChanged { permission },
        }),
        Event::Response(response) => respond(this, response, cx),
        Event::VoicemailFinished(generation) => {
            crate::platform_media::voicemail_finished(generation)
        }
        Event::NetworkChanged | Event::Terminate => {}
    }
    cx.notify();
}

fn respond(this: &mut Oxide, response: Response, cx: &mut Context<Oxide>) {
    let Response {
        request_id,
        call,
        action,
        conversation_id,
        reply_chain_id,
    } = response;
    if call {
        // An old banner must not answer the next call.
        let ringing = this
            .call
            .as_ref()
            .and_then(|c| c.active.as_ref())
            .is_some_and(|a| a.id == request_id);
        if !ringing {
            return;
        }
        match action {
            ResponseAction::Decline => this.send(Action::Call {
                action: CallAction::Decline,
            }),
            ResponseAction::Accept => this.send(Action::Call {
                action: CallAction::Accept { video: false },
            }),
            _ => cx.activate(true),
        }
        return;
    }
    match action {
        ResponseAction::Reply(text) => this.send(Action::Notifications {
            action: NotificationsAction::Reply {
                conversation_id,
                reply_chain_id,
                text,
            },
        }),
        ResponseAction::MarkRead => this.send(Action::Notifications {
            action: NotificationsAction::MarkRead { conversation_id },
        }),
        ResponseAction::Dismiss => {}
        _ => {
            cx.activate(true);
            this.send(Action::Notifications {
                action: NotificationsAction::Open {
                    conversation_id,
                    reply_chain_id,
                },
            });
        }
    }
}

thread_local! {
    static BADGE: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// "3" for unread chats, plus activity: the label the macOS app's `DockBadge` shows.
pub fn dock_badge(unread: Option<(u32, u32)>) {
    let label = badge_label(unread);
    let changed = BADGE.with(|b| b.replace(label.clone()) != label);
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    if changed {
        let tile = NSApplication::sharedApplication(mtm).dockTile();
        tile.setBadgeLabel(label.map(|l| NSString::from_str(&l)).as_deref());
    }
}

fn badge_label(unread: Option<(u32, u32)>) -> Option<String> {
    unread.and_then(|(chats, activity)| {
        let total = chats as u64 + activity as u64;
        (total > 0).then(|| {
            if total > 99 {
                "99+".to_owned()
            } else {
                total.to_string()
            }
        })
    })
}

/// A message banner; its sound is played here, on the chosen speaker.
pub fn notify(banner: Banner) {
    if let Some(sound) = banner.sound {
        crate::platform_media::play_notification_sound(sound);
    }
    let urgent = banner.urgent;
    crate::platform_notifications::show(banner);
    if !is_active() {
        request_attention(urgent);
    }
}

/// Removes the delivered banners of a conversation that has been read.
pub fn withdraw(conversation_id: &str) {
    crate::platform_notifications::withdraw(conversation_id);
}

/// An Accept / Decline banner for an incoming call, under the call's id (`IncomingCallAlert`).
pub fn notify_call(id: &str, title: &str, video: bool) {
    crate::platform_notifications::show_call(id, title, video);
}

/// Removes the incoming call's banner once it stops ringing.
pub fn withdraw_call(id: &str) {
    crate::platform_notifications::remove_call(id);
}

/// The app is frontmost.
pub fn is_active() -> bool {
    MainThreadMarker::new().is_some_and(|mtm| NSApplication::sharedApplication(mtm).isActive())
}

/// Bounces the Dock icon: until the app is activated when `critical`, once otherwise. The id cancels it.
pub fn request_attention(critical: bool) -> Option<isize> {
    let mtm = MainThreadMarker::new()?;
    let kind = if critical {
        NSRequestUserAttentionType::CriticalRequest
    } else {
        NSRequestUserAttentionType::InformationalRequest
    };
    Some(NSApplication::sharedApplication(mtm).requestUserAttention(kind))
}

pub fn cancel_attention(id: isize) {
    if let Some(mtm) = MainThreadMarker::new() {
        NSApplication::sharedApplication(mtm).cancelUserAttentionRequest(id);
    }
}

/// Shows a downloaded file in the Quick Look panel.
pub fn quick_look(path: &str) {
    crate::platform_media::quick_look(path);
}

/// Bounces the Downloads stack in the Dock, as browsers do.
pub fn downloaded(path: &str) {
    let name = NSString::from_str("com.apple.DownloadFileFinished");
    let path = NSString::from_str(path);
    unsafe {
        NSDistributedNotificationCenter::defaultCenter()
            .postNotificationName_object(&name, Some(&path))
    };
}

/// Plays a voicemail the core handed over, one at a time (`VoicemailPlayer`).
pub fn play_voicemail(id: &str, data: Vec<u8>) {
    crate::platform_media::play_voicemail(id, data);
}

#[cfg(test)]
mod tests {
    use super::badge_label;

    #[test]
    fn badge_adds_chats_and_activity() {
        assert_eq!(badge_label(None), None);
        assert_eq!(badge_label(Some((0, 0))), None);
        assert_eq!(badge_label(Some((2, 1))).as_deref(), Some("3"));
        assert_eq!(badge_label(Some((99, 1))).as_deref(), Some("99+"));
        assert_eq!(
            badge_label(Some((u32::MAX, u32::MAX))).as_deref(),
            Some("99+")
        );
    }
}

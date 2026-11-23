//! Windows banners: WinRT toast notifications from this unpackaged exe. The app registers its own identity
//! (AppUserModelID) for the current user, with its name and icon, as Windows asks of apps without a package; a
//! click opens the conversation, a call's buttons answer it.
use std::sync::Mutex;
use teams_core::{Action, CallAction, NotificationsAction};
use windows::Data::Xml::Dom::XmlDocument;
use windows::Foundation::TypedEventHandler;
use windows::UI::Notifications::{
    ToastActivatedEventArgs, ToastNotification, ToastNotificationManager, ToastNotifier,
};
use windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID;
use windows::core::{HSTRING, IInspectable, Interface};

const AUMID: &str = "fr.neicureuil.oxide.gpui";
/// Banners kept so their click handlers stay registered; older ones are in the Action Center only.
const KEPT: usize = 30;

/// What a click on a banner asks of the app, sent to the window's task.
pub enum Click {
    Open {
        conversation_id: String,
        reply_chain_id: Option<String>,
    },
    Call {
        id: String,
        action: Option<CallAction>,
    },
}

struct State {
    notifier: ToastNotifier,
    shown: Vec<ToastNotification>,
    clicks: futures::channel::mpsc::UnboundedSender<Click>,
}

// SAFETY: WinRT objects here are agile (usable from any thread); they're only touched behind the lock.
unsafe impl Send for State {}

static STATE: Mutex<Option<State>> = Mutex::new(None);

/// Registers the app's identity and starts sending clicks to `clicks`.
pub fn start(clicks: futures::channel::mpsc::UnboundedSender<Click>) {
    if let Err(error) = register() {
        crate::media_engine::trace(&format!("notifications: registration failed: {error}"));
    }
    match ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(AUMID)) {
        Ok(notifier) => {
            *STATE.lock().unwrap() = Some(State {
                notifier,
                shown: Vec::new(),
                clicks,
            })
        }
        Err(error) => crate::media_engine::trace(&format!("notifications: no notifier: {error}")),
    }
}

/// `HKCU\Software\Classes\AppUserModelId\<id>`: the name and icon Windows shows on the banners.
fn register() -> windows::core::Result<()> {
    let key = windows_registry::CURRENT_USER
        .create(format!("Software\\Classes\\AppUserModelId\\{AUMID}"))?;
    key.set_string("DisplayName", "Oxide")?;
    if let Some(icon) = icon_file() {
        key.set_string("IconUri", icon.to_string_lossy().as_ref())?;
    }
    // SAFETY: a plain Win32 call; it also groups the windows under this identity in the taskbar.
    unsafe { SetCurrentProcessExplicitAppUserModelID(&HSTRING::from(AUMID))? };
    Ok(())
}

/// The 256-pixel picture of the exe's icon, written next to the caches for Windows to read.
fn icon_file() -> Option<std::path::PathBuf> {
    let ico: &[u8] = include_bytes!("../resources/oxide.ico");
    let count = u16::from_le_bytes([ico[4], ico[5]]) as usize;
    let largest = (0..count).find(|i| ico[6 + 16 * i] == 0)?;
    let entry = &ico[6 + 16 * largest..6 + 16 * largest + 16];
    let size = u32::from_le_bytes(entry[8..12].try_into().ok()?) as usize;
    let offset = u32::from_le_bytes(entry[12..16].try_into().ok()?) as usize;
    let png = ico.get(offset..offset + size)?;
    let path = crate::paths::caches()?.join("notification-icon.png");
    if std::fs::read(&path).ok().as_deref() != Some(png) {
        std::fs::create_dir_all(path.parent()?).ok()?;
        std::fs::write(&path, png).ok()?;
    }
    Some(path)
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// Toast tags and groups take at most 64 characters: ids are hashed down to 16.
fn short(id: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    id.hash(&mut hasher);
    format!("{:016x}", hasher.finish())
}

fn show(
    xml: &str,
    tag: &str,
    group: &str,
    on_click: impl Fn(Option<String>) -> Option<Click> + Send + 'static,
) {
    let mut state = STATE.lock().unwrap();
    let Some(state) = state.as_mut() else {
        return;
    };
    let shown = (|| -> windows::core::Result<ToastNotification> {
        let document = XmlDocument::new()?;
        document.LoadXml(&HSTRING::from(xml))?;
        let toast = ToastNotification::CreateToastNotification(&document)?;
        toast.SetTag(&HSTRING::from(tag))?;
        toast.SetGroup(&HSTRING::from(group))?;
        let clicks = state.clicks.clone();
        toast.Activated(&TypedEventHandler::<ToastNotification, IInspectable>::new(
            move |_, args| {
                let arguments = args
                    .as_ref()
                    .and_then(|a| a.cast::<ToastActivatedEventArgs>().ok())
                    .and_then(|a| a.Arguments().ok())
                    .map(|a| a.to_string())
                    .filter(|a| !a.is_empty());
                if let Some(click) = on_click(arguments) {
                    let _ = clicks.unbounded_send(click);
                }
                Ok(())
            },
        ))?;
        state.notifier.Show(&toast)?;
        Ok(toast)
    })();
    match shown {
        Ok(toast) => {
            state.shown.push(toast);
            if state.shown.len() > KEPT {
                state.shown.remove(0);
            }
        }
        Err(error) => crate::media_engine::trace(&format!("notifications: not shown: {error}")),
    }
}

/// A message banner: the system plays its own sound unless the core asked for none.
pub fn notify(banner: &crate::platform::Banner) {
    let lines = [
        Some(&banner.title),
        banner.subtitle.as_ref(),
        Some(&banner.body),
    ]
    .into_iter()
    .flatten()
    .filter(|l| !l.is_empty())
    .map(|l| format!("<text>{}</text>", escape(l)))
    .collect::<String>();
    let audio = if banner.sound.is_some() {
        ""
    } else {
        r#"<audio silent="true"/>"#
    };
    let xml = format!(
        r#"<toast launch="open"><visual><binding template="ToastGeneric">{lines}</binding></visual>{audio}</toast>"#
    );
    let (conversation_id, reply_chain_id) = (
        banner.conversation_id.clone(),
        banner.reply_chain_id.clone(),
    );
    show(
        &xml,
        &short(&banner.id),
        &short(&banner.conversation_id),
        move |_| {
            Some(Click::Open {
                conversation_id: conversation_id.clone(),
                reply_chain_id: reply_chain_id.clone(),
            })
        },
    );
}

/// Removes the banners of a conversation that has been read.
pub fn withdraw(conversation_id: &str) {
    if let Ok(history) = ToastNotificationManager::History() {
        let _ = history.RemoveGroupWithId(
            &HSTRING::from(short(conversation_id)),
            &HSTRING::from(AUMID),
        );
    }
}

const CALLS: &str = "calls";

/// An incoming call, ringing until answered, with Accept / Decline.
pub fn notify_call(id: &str, title: &str, video: bool) {
    use crate::oxide::tr;
    let body = tr(if video {
        "ui.notification_router.incoming_video_call"
    } else {
        "ui.notification_router.incoming_audio_call"
    });
    let xml = format!(
        r#"<toast launch="open" scenario="incomingCall"><visual><binding template="ToastGeneric"><text>{}</text><text>{}</text></binding></visual><actions><action content="{}" arguments="accept"/><action content="{}" arguments="decline"/></actions></toast>"#,
        escape(title),
        escape(&body),
        escape(&tr("ui.notification_router.accept")),
        escape(&tr("ui.notification_router.decline")),
    );
    let call = id.to_owned();
    show(&xml, &short(id), CALLS, move |arguments| {
        let action = match arguments.as_deref() {
            Some("accept") => Some(CallAction::Accept { video: false }),
            Some("decline") => Some(CallAction::Decline),
            _ => None,
        };
        Some(Click::Call {
            id: call.clone(),
            action,
        })
    });
}

pub fn withdraw_call(id: &str) {
    if let Ok(history) = ToastNotificationManager::History() {
        let _ = history.RemoveGroupedTagWithId(
            &HSTRING::from(short(id)),
            &HSTRING::from(CALLS),
            &HSTRING::from(AUMID),
        );
    }
}

/// A click, on the window's task: as `platform::respond` does on macOS.
pub fn handle(
    this: &mut crate::oxide::Oxide,
    click: Click,
    cx: &mut gpui::Context<crate::oxide::Oxide>,
) {
    match click {
        Click::Open {
            conversation_id,
            reply_chain_id,
        } => {
            cx.activate(true);
            this.send(Action::Notifications {
                action: NotificationsAction::Open {
                    conversation_id,
                    reply_chain_id,
                },
            });
        }
        Click::Call { id, action } => {
            // An old banner must not answer the next call.
            let ringing = this
                .call
                .as_ref()
                .and_then(|c| c.active.as_ref())
                .is_some_and(|a| a.id == id);
            match action {
                Some(action) if ringing => this.send(Action::Call { action }),
                _ => cx.activate(true),
            }
        }
    }
}

/// Our window is the one in front.
pub fn is_active() -> bool {
    use windows::Win32::System::Threading::GetCurrentProcessId;
    use windows::Win32::UI::WindowsAndMessaging::{GetForegroundWindow, GetWindowThreadProcessId};
    // SAFETY: plain Win32 queries.
    unsafe {
        let window = GetForegroundWindow();
        let mut process = 0;
        GetWindowThreadProcessId(window, Some(&mut process));
        process == GetCurrentProcessId()
    }
}

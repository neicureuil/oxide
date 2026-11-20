//! Banners through UNUserNotificationCenter (`NotificationRouter.swift`): messages with quick reply, incoming calls,
//! and the permission. The center only exists in a bundled app: a bare `cargo run` shows no banner.
use crate::oxide::tr;
use crate::platform::{Banner, Event, Response, ResponseAction, post};
use block2::RcBlock;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Bool, NSObjectProtocol, ProtocolObject};
use objc2::{AnyThread, define_class, msg_send};
use objc2_app_kit::NSWorkspace;
use objc2_foundation::{
    NSArray, NSBundle, NSDictionary, NSError, NSObject, NSSet, NSString, NSURL,
};
use objc2_user_notifications::{
    UNAuthorizationOptions, UNAuthorizationStatus, UNMutableNotificationContent, UNNotification,
    UNNotificationAction, UNNotificationActionOptions, UNNotificationCategory,
    UNNotificationCategoryOptions, UNNotificationDefaultActionIdentifier,
    UNNotificationDismissActionIdentifier, UNNotificationInterruptionLevel,
    UNNotificationPresentationOptions, UNNotificationRequest, UNNotificationResponse,
    UNNotificationSettings, UNTextInputNotificationAction, UNTextInputNotificationResponse,
    UNUserNotificationCenter, UNUserNotificationCenterDelegate,
};
use std::cell::{Cell, RefCell};
use std::ptr::NonNull;
use std::sync::Mutex;
use teams_core::NotificationPermission;

const CALL_CATEGORY: &str = "INCOMING_CALL";
const CALL_ACCEPT: &str = "ACCEPT";
const CALL_DECLINE: &str = "DECLINE";
const MESSAGE_CATEGORY: &str = "MESSAGE";
const MESSAGE_REPLY_CATEGORY: &str = "MESSAGE_REPLY";
const REPLY: &str = "REPLY";
const MARK_READ: &str = "MARK_READ";

define_class!(
    /// The only notification center delegate (messages and calls).
    #[unsafe(super(NSObject))]
    #[name = "OxideNotificationRouter"]
    struct Router;

    unsafe impl NSObjectProtocol for Router {}

    unsafe impl UNUserNotificationCenterDelegate for Router {
        /// The core already left out banners for the conversation on screen.
        #[unsafe(method(userNotificationCenter:willPresentNotification:withCompletionHandler:))]
        fn will_present(
            &self,
            _center: &UNUserNotificationCenter,
            _notification: &UNNotification,
            handler: &block2::DynBlock<dyn Fn(UNNotificationPresentationOptions)>,
        ) {
            handler.call((UNNotificationPresentationOptions::Banner
                | UNNotificationPresentationOptions::List
                | UNNotificationPresentationOptions::Sound,));
        }

        #[unsafe(method(userNotificationCenter:didReceiveNotificationResponse:withCompletionHandler:))]
        fn did_receive(
            &self,
            _center: &UNUserNotificationCenter,
            response: &UNNotificationResponse,
            handler: &block2::DynBlock<dyn Fn()>,
        ) {
            post(Event::Response(decode(response)));
            handler.call(());
        }
    }
);

impl Router {
    fn new() -> Retained<Self> {
        let this = Self::alloc().set_ivars(());
        unsafe { msg_send![super(this), init] }
    }
}

fn decode(response: &UNNotificationResponse) -> Response {
    let action = response.actionIdentifier().to_string();
    let request = response.notification().request();
    let content = request.content();
    let call = content.categoryIdentifier().to_string() == CALL_CATEGORY;
    let info = content.userInfo();
    let value = |key: &str| {
        let key = NSString::from_str(key);
        info.objectForKey(&key)
            .and_then(|v| v.downcast::<NSString>().ok())
            .map(|v| v.to_string())
            .unwrap_or_default()
    };
    let chain = value("replyChain");
    let action = if action == REPLY {
        let text = (response as &AnyObject)
            .downcast_ref::<UNTextInputNotificationResponse>()
            .map(|r| r.userText().to_string())
            .unwrap_or_default();
        ResponseAction::Reply(text)
    } else if action == MARK_READ {
        ResponseAction::MarkRead
    } else if action == CALL_ACCEPT {
        ResponseAction::Accept
    } else if action == CALL_DECLINE {
        ResponseAction::Decline
    } else if action == unsafe { UNNotificationDismissActionIdentifier }.to_string() {
        ResponseAction::Dismiss
    } else {
        let _ = unsafe { UNNotificationDefaultActionIdentifier };
        ResponseAction::Open
    };
    Response {
        request_id: request.identifier().to_string(),
        call,
        action,
        conversation_id: value("conversation"),
        reply_chain_id: (!chain.is_empty()).then_some(chain),
    }
}

thread_local! {
    static ROUTER: RefCell<Option<Retained<Router>>> = const { RefCell::new(None) };
    /// Language the categories were registered in: their titles follow the app language.
    static CATEGORIES_ENGLISH: Cell<Option<bool>> = const { Cell::new(None) };
}
static PERMISSION: Mutex<NotificationPermission> = Mutex::new(NotificationPermission::Unknown);

/// Notifications need a real app bundle: `currentNotificationCenter` throws without one.
fn center() -> Option<Retained<UNUserNotificationCenter>> {
    NSBundle::mainBundle().bundleIdentifier()?;
    Some(UNUserNotificationCenter::currentNotificationCenter())
}

/// Sets the delegate once, and registers the categories again when the language changed.
pub(crate) fn configure() {
    let Some(center) = center() else { return };
    ROUTER.with(|router| {
        let mut router = router.borrow_mut();
        if router.is_none() {
            let made = Router::new();
            center.setDelegate(Some(ProtocolObject::from_ref(&*made)));
            // The delegate is weak: kept here for the app's life.
            *router = Some(made);
        }
    });
    let english = crate::oxide::english();
    if CATEGORIES_ENGLISH.with(|c| c.replace(Some(english))) == Some(english) {
        return;
    }
    let s = NSString::from_str;
    let accept = UNNotificationAction::actionWithIdentifier_title_options(
        &s(CALL_ACCEPT),
        &s(&tr("ui.notification_router.accept")),
        UNNotificationActionOptions::Foreground,
    );
    let decline = UNNotificationAction::actionWithIdentifier_title_options(
        &s(CALL_DECLINE),
        &s(&tr("ui.notification_router.decline")),
        UNNotificationActionOptions::Destructive,
    );
    let reply =
        UNTextInputNotificationAction::actionWithIdentifier_title_options_textInputButtonTitle_textInputPlaceholder(
            &s(REPLY),
            &s(&tr("ui.notification_router.reply")),
            UNNotificationActionOptions::empty(),
            &s(&tr("ui.notification_router.send")),
            &s(&tr("ui.notification_router.your_reply")),
        );
    let read = UNNotificationAction::actionWithIdentifier_title_options(
        &s(MARK_READ),
        &s(&tr("ui.notification_router.mark_as_read")),
        UNNotificationActionOptions::empty(),
    );
    let category = |id: &str, actions: &[&UNNotificationAction]| {
        UNNotificationCategory::categoryWithIdentifier_actions_intentIdentifiers_options(
            &s(id),
            &NSArray::from_slice(actions),
            &NSArray::new(),
            UNNotificationCategoryOptions::empty(),
        )
    };
    let categories = [
        category(CALL_CATEGORY, &[&accept, &decline]),
        category(MESSAGE_REPLY_CATEGORY, &[&reply, &read]),
        category(MESSAGE_CATEGORY, &[&read]),
    ];
    let categories: Vec<&UNNotificationCategory> = categories.iter().map(|c| &**c).collect();
    center.setNotificationCategories(&NSSet::from_slice(&categories));
}

/// Asks once (the system remembers the answer), then reports the state to the core.
pub fn request_permission() {
    let Some(center) = center() else {
        set_permission(NotificationPermission::Unknown);
        return;
    };
    configure();
    let done = RcBlock::new(|_granted: Bool, _error: *mut NSError| refresh_permission());
    center.requestAuthorizationWithOptions_completionHandler(
        UNAuthorizationOptions::Alert
            | UNAuthorizationOptions::Sound
            | UNAuthorizationOptions::Badge,
        &done,
    );
}

/// The user may change it in System Settings at any time: read again when the app comes to the front.
pub fn refresh_permission() {
    let Some(center) = center() else {
        set_permission(NotificationPermission::Unknown);
        return;
    };
    let done = RcBlock::new(|settings: NonNull<UNNotificationSettings>| {
        let status = unsafe { settings.as_ref() }.authorizationStatus();
        set_permission(match status {
            UNAuthorizationStatus::Authorized
            | UNAuthorizationStatus::Provisional
            | UNAuthorizationStatus::Ephemeral => NotificationPermission::Granted,
            UNAuthorizationStatus::Denied => NotificationPermission::Denied,
            _ => NotificationPermission::Unknown,
        });
    });
    center.getNotificationSettingsWithCompletionHandler(&done);
}

fn set_permission(permission: NotificationPermission) {
    *PERMISSION.lock().unwrap() = permission;
    post(Event::Permission(permission));
}

/// The system's answer, as last read; the core's `NotificationsView.permission` carries it too.
pub fn permission() -> NotificationPermission {
    *PERMISSION.lock().unwrap()
}

/// System Settings › Notifications, on this app when possible.
pub fn open_notification_settings() {
    let id = NSBundle::mainBundle()
        .bundleIdentifier()
        .map(|id| id.to_string())
        .unwrap_or_default();
    let url =
        format!("x-apple.systempreferences:com.apple.Notifications-Settings.extension?id={id}");
    if let Some(url) = NSURL::URLWithString(&NSString::from_str(&url)) {
        NSWorkspace::sharedWorkspace().openURL(&url);
    }
}

fn user_info(conversation: &str, chain: &str) -> Retained<NSDictionary<NSString, NSString>> {
    let keys = [
        NSString::from_str("conversation"),
        NSString::from_str("replyChain"),
    ];
    let values = [NSString::from_str(conversation), NSString::from_str(chain)];
    NSDictionary::from_slices(&[&*keys[0], &*keys[1]], &[&*values[0], &*values[1]])
}

pub(crate) fn show(banner: Banner) {
    let Some(center) = center() else { return };
    configure();
    let content = UNMutableNotificationContent::new();
    content.setTitle(&NSString::from_str(&banner.title));
    if let Some(subtitle) = &banner.subtitle {
        content.setSubtitle(&NSString::from_str(subtitle));
    }
    content.setBody(&NSString::from_str(&banner.body));
    content.setThreadIdentifier(&NSString::from_str(&banner.conversation_id));
    content.setCategoryIdentifier(&NSString::from_str(if banner.can_reply {
        MESSAGE_REPLY_CATEGORY
    } else {
        MESSAGE_CATEGORY
    }));
    let info = user_info(
        &banner.conversation_id,
        banner.reply_chain_id.as_deref().unwrap_or(""),
    );
    // Strings to strings: a valid property list for `userInfo`.
    unsafe { content.setUserInfo(Retained::cast_unchecked::<NSDictionary>(info).as_ref()) };
    content.setInterruptionLevel(if banner.urgent {
        UNNotificationInterruptionLevel::TimeSensitive
    } else {
        UNNotificationInterruptionLevel::Active
    });
    // The sound is played by the app, on the chosen speaker, so the banner itself is silent.
    content.setSound(None);
    add(&center, &banner.id, &content);
}

fn add(center: &UNUserNotificationCenter, id: &str, content: &UNMutableNotificationContent) {
    let request = UNNotificationRequest::requestWithIdentifier_content_trigger(
        &NSString::from_str(id),
        content,
        None,
    );
    center.addNotificationRequest_withCompletionHandler(&request, None);
}

pub(crate) fn withdraw(conversation_id: &str) {
    let Some(center) = center() else { return };
    let conversation = conversation_id.to_owned();
    let found = RcBlock::new(move |delivered: NonNull<NSArray<UNNotification>>| {
        let ids: Vec<Retained<NSString>> = unsafe { delivered.as_ref() }
            .iter()
            .map(|n| n.request())
            .filter(|r| r.content().threadIdentifier().to_string() == conversation)
            .map(|r| r.identifier())
            .collect();
        if let (false, Some(center)) = (ids.is_empty(), self::center()) {
            let ids: Vec<&NSString> = ids.iter().map(|i| &**i).collect();
            center.removeDeliveredNotificationsWithIdentifiers(&NSArray::from_slice(&ids));
        }
    });
    center.getDeliveredNotificationsWithCompletionHandler(&found);
}

pub(crate) fn show_call(id: &str, title: &str, video: bool) {
    let Some(center) = center() else { return };
    configure();
    let content = UNMutableNotificationContent::new();
    content.setTitle(&NSString::from_str(title));
    content.setBody(&NSString::from_str(&tr(if video {
        "ui.notification_router.incoming_video_call"
    } else {
        "ui.notification_router.incoming_audio_call"
    })));
    content.setCategoryIdentifier(&NSString::from_str(CALL_CATEGORY));
    content.setInterruptionLevel(UNNotificationInterruptionLevel::TimeSensitive);
    add(&center, id, &content);
}

pub(crate) fn remove_call(id: &str) {
    let Some(center) = center() else { return };
    let ids = NSArray::from_retained_slice(&[NSString::from_str(id)]);
    center.removeDeliveredNotificationsWithIdentifiers(&ids);
    center.removePendingNotificationRequestsWithIdentifiers(&ids);
}

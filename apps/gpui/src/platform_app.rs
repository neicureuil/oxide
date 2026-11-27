//! The `AppDelegate` parts GPUI leaves out (`App.swift`): hanging up before quitting, re-reading the
//! notification permission on activation, and VoiceOver announcements.
use crate::platform::{Event, post};
use block2::RcBlock;
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, Imp, Sel};
use objc2::sel;
use objc2_app_kit::{
    NSAccessibilityAnnouncementKey, NSAccessibilityAnnouncementRequestedNotification,
    NSAccessibilityPostNotificationWithUserInfo, NSAccessibilityPriorityKey,
    NSAccessibilityPriorityLevel, NSApplication, NSApplicationDidBecomeActiveNotification,
    NSApplicationTerminateReply,
};
use objc2_foundation::{NSDictionary, NSNotification, NSNotificationCenter, NSNumber, NSString};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, Ordering};

static IN_CALL: AtomicBool = AtomicBool::new(false);
/// Set once the call is hung up: the next quit request goes through.
static HUNG_UP: AtomicBool = AtomicBool::new(false);

pub(crate) fn set_in_call(in_call: bool) {
    IN_CALL.store(in_call, Ordering::Relaxed);
}

/// `applicationShouldTerminate:`, added to GPUI's app delegate: every quit (menu, Dock, ⌘Q, last window,
/// log out) goes through it. Mid-call, the quit is put off until the window's task has hung up, then asked
/// again. `NSTerminateLater` would be the AppKit way, but it runs a modal loop in which GPUI's tasks never run.
extern "C-unwind" fn should_terminate(
    _this: &AnyObject,
    _sel: Sel,
    _sender: *mut AnyObject,
) -> NSApplicationTerminateReply {
    if IN_CALL.load(Ordering::Relaxed) && !HUNG_UP.load(Ordering::Relaxed) && post(Event::Terminate)
    {
        NSApplicationTerminateReply::TerminateCancel
    } else {
        NSApplicationTerminateReply::TerminateNow
    }
}

pub(crate) fn install() {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let app = NSApplication::sharedApplication(mtm);
    if let Some(delegate) = app.delegate() {
        let object: &AnyObject = (*delegate).as_ref();
        let class = object.class();
        let imp: Imp = unsafe {
            std::mem::transmute::<
                extern "C-unwind" fn(
                    &AnyObject,
                    Sel,
                    *mut AnyObject,
                ) -> NSApplicationTerminateReply,
                Imp,
            >(should_terminate)
        };
        // NSUInteger (id self, SEL _cmd, id sender); a no-op if GPUI ever implements it.
        unsafe {
            objc2::ffi::class_addMethod(
                class as *const _ as *mut _,
                sel!(applicationShouldTerminate:),
                imp,
                c"Q@:@".as_ptr(),
            )
        };
    }

    // Notifications may have been allowed or refused in System Settings meanwhile.
    let became_active =
        RcBlock::new(|_: NonNull<NSNotification>| crate::platform::refresh_permission());
    let token: Retained<_> = unsafe {
        NSNotificationCenter::defaultCenter().addObserverForName_object_queue_usingBlock(
            Some(NSApplicationDidBecomeActiveNotification),
            None,
            None,
            &became_active,
        )
    };
    std::mem::forget(token);
}

/// Quits again, now that the call is hung up (or the 3 s are over).
pub(crate) fn quit_now(cx: &mut gpui::AsyncApp) {
    HUNG_UP.store(true, Ordering::Relaxed);
    let _ = cx.update(|cx| cx.quit());
}

/// Read by VoiceOver at once, as the offline banner appears or goes.
pub(crate) fn announce(text: &str) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let app = NSApplication::sharedApplication(mtm);
    let text = NSString::from_str(text);
    let priority = NSNumber::new_isize(NSAccessibilityPriorityLevel::High.0);
    unsafe {
        let info = NSDictionary::<NSString, AnyObject>::from_slices(
            &[NSAccessibilityAnnouncementKey, NSAccessibilityPriorityKey],
            &[&*text as &AnyObject, &*priority as &AnyObject],
        );
        NSAccessibilityPostNotificationWithUserInfo(
            &app,
            NSAccessibilityAnnouncementRequestedNotification,
            Some(&info),
        );
    }
}

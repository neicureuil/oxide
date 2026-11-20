//! Network path and wake-ups (`NetworkWatcher.swift`), through Network.framework's C API: the offline banner,
//! and a reconnect of push and call media without waiting for timeouts.
use crate::platform::{Event, post};
use block2::RcBlock;
use dispatch2::DispatchQueue;
use objc2::rc::Retained;
use objc2::runtime::Bool;
use objc2_app_kit::{NSWorkspace, NSWorkspaceDidWakeNotification};
use objc2_foundation::NSNotification;
use std::ffi::{CStr, c_char, c_int, c_void};
use std::ptr::NonNull;
use std::sync::{Arc, Mutex};

type Object = *mut c_void;

#[link(name = "Network", kind = "framework")]
unsafe extern "C" {
    fn nw_path_monitor_create() -> Object;
    fn nw_path_monitor_set_queue(monitor: Object, queue: *const DispatchQueue);
    fn nw_path_monitor_set_update_handler(
        monitor: Object,
        handler: &block2::DynBlock<dyn Fn(Object)>,
    );
    fn nw_path_monitor_start(monitor: Object);
    fn nw_path_get_status(path: Object) -> c_int;
    fn nw_path_enumerate_interfaces(path: Object, each: &block2::DynBlock<dyn Fn(Object) -> Bool>);
    fn nw_interface_get_name(interface: Object) -> *const c_char;
}

/// `nw_path_status_satisfied`.
const SATISFIED: c_int = 1;

/// Interfaces in use and whether the path is usable, as Swift's `NetworkWatcher.signature`.
fn signature(path: Object) -> (bool, String) {
    let satisfied = unsafe { nw_path_get_status(path) } == SATISFIED;
    let names = Arc::new(Mutex::new(Vec::<String>::new()));
    let found = names.clone();
    let each = RcBlock::new(move |interface: Object| {
        let name = unsafe { nw_interface_get_name(interface) };
        if !name.is_null() {
            let name = unsafe { CStr::from_ptr(name) }
                .to_string_lossy()
                .into_owned();
            found.lock().unwrap().push(name);
        }
        Bool::YES
    });
    unsafe { nw_path_enumerate_interfaces(path, &each) };
    drop(each);
    let names = names.lock().unwrap().join(",");
    (satisfied, names)
}

/// Reports every path update, and a change once the first (baseline) is known; kept for the app's life.
pub(crate) fn start() {
    let last: Mutex<Option<(bool, String)>> = Mutex::new(None);
    let update = RcBlock::new(move |path: Object| {
        let now = signature(path);
        post(Event::Online(now.0));
        let mut last = last.lock().unwrap();
        if now.0 && last.as_ref().is_some_and(|l| *l != now) {
            post(Event::NetworkChanged);
        }
        *last = Some(now);
    });
    let queue = DispatchQueue::new("oxide.network-watcher", None);
    unsafe {
        let monitor = nw_path_monitor_create();
        nw_path_monitor_set_update_handler(monitor, &update);
        nw_path_monitor_set_queue(monitor, &*queue);
        nw_path_monitor_start(monitor);
    }
    // The monitor copies the block and retains the queue; the monitor itself is never released.
    std::mem::forget(queue);

    let woke = RcBlock::new(|_: NonNull<NSNotification>| {
        post(Event::NetworkChanged);
    });
    let token: Retained<_> = unsafe {
        NSWorkspace::sharedWorkspace()
            .notificationCenter()
            .addObserverForName_object_queue_usingBlock(
                Some(NSWorkspaceDidWakeNotification),
                None,
                None,
                &woke,
            )
    };
    std::mem::forget(token);
}

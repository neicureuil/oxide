//! The Windows sign-in window: a plain Win32 window holding WebView2. GPUI draws its windows through a
//! DirectComposition visual kept above child windows, so a web view inside a GPUI window stays hidden
//! behind it (a white page). In a window of its own, wry sizes the web view to the window.
use std::cell::Cell;
use std::ffi::c_void;
use std::num::NonZeroIsize;
use wry::WebView;
use wry::raw_window_handle::{
    HandleError, HasWindowHandle, RawWindowHandle, Win32WindowHandle, WindowHandle,
};

type Hwnd = *mut c_void;
type WndProc = unsafe extern "system" fn(Hwnd, u32, usize, isize) -> isize;

#[repr(C)]
struct WndClassExW {
    size: u32,
    style: u32,
    proc_: Option<WndProc>,
    class_extra: i32,
    window_extra: i32,
    instance: *mut c_void,
    icon: *mut c_void,
    cursor: *mut c_void,
    background: *mut c_void,
    menu_name: *const u16,
    class_name: *const u16,
    small_icon: *mut c_void,
}

#[link(name = "user32")]
unsafe extern "system" {
    fn RegisterClassExW(class: *const WndClassExW) -> u16;
    fn CreateWindowExW(
        ex_style: u32,
        class: *const u16,
        title: *const u16,
        style: u32,
        x: i32,
        y: i32,
        width: i32,
        height: i32,
        parent: Hwnd,
        menu: *mut c_void,
        instance: *mut c_void,
        param: *mut c_void,
    ) -> Hwnd;
    fn DefWindowProcW(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize;
    fn DestroyWindow(hwnd: Hwnd) -> i32;
    fn ShowWindow(hwnd: Hwnd, cmd: i32) -> i32;
    fn SetForegroundWindow(hwnd: Hwnd) -> i32;
    fn LoadCursorW(instance: *mut c_void, name: *const u16) -> *mut c_void;
    fn SetWindowLongPtrW(hwnd: Hwnd, index: i32, value: isize) -> isize;
    fn GetWindowLongPtrW(hwnd: Hwnd, index: i32) -> isize;
    fn GetDpiForSystem() -> u32;
    fn GetSystemMetrics(index: i32) -> i32;
}
#[link(name = "kernel32")]
unsafe extern "system" {
    fn GetModuleHandleW(name: *const u16) -> *mut c_void;
}

const WM_DESTROY: u32 = 0x0002;
const WM_NCDESTROY: u32 = 0x0082;
const WS_OVERLAPPEDWINDOW: u32 = 0x00CF_0000;
const SW_SHOW: i32 = 5;
const GWLP_USERDATA: i32 = -21;
const IDC_ARROW: usize = 32512;
const COLOR_WINDOW: usize = 5;
const SM_CXSCREEN: i32 = 0;
const SM_CYSCREEN: i32 = 1;

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}

/// What the window procedure reaches through `GWLP_USERDATA`.
struct State {
    hwnd: Cell<Hwnd>,
    web: Cell<Option<WebView>>,
    /// Told once the window is gone, whoever closed it.
    on_closed: Cell<Option<Box<dyn FnOnce()>>>,
}

unsafe extern "system" fn window_proc(hwnd: Hwnd, msg: u32, wparam: usize, lparam: isize) -> isize {
    // SAFETY: set to a `State` that outlives the window (`LoginWindow` destroys the window before freeing it).
    let state = unsafe { (GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *const State).as_ref() };
    match (msg, state) {
        (WM_DESTROY, Some(state)) => {
            // The web view goes before its window, then the store learns it closed.
            drop(state.web.take());
            if let Some(on_closed) = state.on_closed.take() {
                on_closed();
            }
        }
        (WM_NCDESTROY, Some(state)) => {
            state.hwnd.set(std::ptr::null_mut());
            unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0) };
        }
        _ => {}
    }
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

fn class_name() -> Option<Vec<u16>> {
    static REGISTERED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    let name = wide("OxideSignIn");
    let registered = *REGISTERED.get_or_init(|| {
        let class = WndClassExW {
            size: std::mem::size_of::<WndClassExW>() as u32,
            style: 0,
            proc_: Some(window_proc),
            class_extra: 0,
            window_extra: 0,
            // SAFETY: plain Win32 calls with valid arguments.
            instance: unsafe { GetModuleHandleW(std::ptr::null()) },
            icon: std::ptr::null_mut(),
            cursor: unsafe { LoadCursorW(std::ptr::null_mut(), IDC_ARROW as *const u16) },
            background: (COLOR_WINDOW + 1) as *mut c_void,
            menu_name: std::ptr::null(),
            class_name: name.as_ptr(),
            small_icon: std::ptr::null_mut(),
        };
        unsafe { RegisterClassExW(&class) != 0 }
    });
    registered.then_some(name)
}

/// The window's handle, as wry takes it.
pub struct Parent(Hwnd);

impl HasWindowHandle for Parent {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let hwnd = NonZeroIsize::new(self.0 as isize).ok_or(HandleError::Unavailable)?;
        // SAFETY: the window lives while the web view is built in it.
        Ok(unsafe {
            WindowHandle::borrow_raw(RawWindowHandle::Win32(Win32WindowHandle::new(hwnd)))
        })
    }
}

/// An open sign-in window; dropping it closes it.
pub struct LoginWindow {
    state: Box<State>,
}

impl LoginWindow {
    /// A centered window of `width`×`height` logical pixels, its web view made by `build`.
    pub fn open(
        title: &str,
        width: f32,
        height: f32,
        build: impl FnOnce(&Parent) -> wry::Result<WebView>,
        on_closed: impl FnOnce() + 'static,
    ) -> Result<Self, String> {
        let class = class_name().ok_or("the sign-in window class could not be registered")?;
        // SAFETY: plain Win32 calls; the window is destroyed by `Drop` or by the person closing it.
        let scale = unsafe { GetDpiForSystem() }.max(96) as f32 / 96.;
        let (w, h) = ((width * scale) as i32, (height * scale) as i32);
        let (screen_w, screen_h) =
            unsafe { (GetSystemMetrics(SM_CXSCREEN), GetSystemMetrics(SM_CYSCREEN)) };
        let title = wide(title);
        let hwnd = unsafe {
            CreateWindowExW(
                0,
                class.as_ptr(),
                title.as_ptr(),
                WS_OVERLAPPEDWINDOW,
                ((screen_w - w) / 2).max(0),
                ((screen_h - h) / 2).max(0),
                w,
                h,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                GetModuleHandleW(std::ptr::null()),
                std::ptr::null_mut(),
            )
        };
        if hwnd.is_null() {
            return Err("the sign-in window could not be created".into());
        }
        let state = Box::new(State {
            hwnd: Cell::new(hwnd),
            web: Cell::new(None),
            on_closed: Cell::new(None),
        });
        unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, &*state as *const State as isize) };
        let window = Self { state };
        let web = build(&Parent(hwnd)).map_err(|error| error.to_string())?;
        window.state.web.set(Some(web));
        window.state.on_closed.set(Some(Box::new(on_closed)));
        unsafe {
            ShowWindow(hwnd, SW_SHOW);
            SetForegroundWindow(hwnd);
        }
        Ok(window)
    }

    /// Runs `f` on the web view while the window is open.
    pub fn with_web(&self, f: impl FnOnce(&WebView)) {
        if let Some(web) = self.state.web.take() {
            f(&web);
            self.state.web.set(Some(web));
        }
    }
}

impl Drop for LoginWindow {
    fn drop(&mut self) {
        let hwnd = self.state.hwnd.get();
        if !hwnd.is_null() {
            // SAFETY: still a window of ours; its procedure reaches `state`, freed only after this.
            unsafe { DestroyWindow(hwnd) };
        }
    }
}

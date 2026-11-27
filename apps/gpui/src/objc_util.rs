//! Small helpers to call AppKit, AVFoundation and Foundation through the Objective-C runtime (macOS only).
//! Objects are handled as `AnyObject`: the code mirrors the Swift app call for call, without bindings.
use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyClass, AnyObject};
use objc2::{Encoding, msg_send};
use objc2_foundation::NSString;
use std::ffi::CStr;

pub type Obj = Retained<AnyObject>;

/// Opaque C types that Objective-C methods take or return by pointer, named as their type encodings name them.
macro_rules! opaque {
    ($($name:ident = $c:literal),* $(,)?) => {$(
        #[repr(C)]
        pub struct $name {
            _private: [u8; 0],
        }
        unsafe impl objc2::RefEncode for $name {
            const ENCODING_REF: Encoding = Encoding::Pointer(&Encoding::Struct($c, &[]));
        }
    )*};
}
opaque!(
    CVBuffer = "__CVBuffer",
    CMSampleBuffer = "opaqueCMSampleBuffer",
    AudioUnit = "ComponentInstanceRecord",
);

/// A class by runtime name; `None` when its framework is not loaded.
pub fn class(name: &CStr) -> Option<&'static AnyClass> {
    AnyClass::get(name)
}

pub fn alloc(name: &CStr) -> Option<Allocated<AnyObject>> {
    let cls = class(name)?;
    Some(unsafe { msg_send![cls, alloc] })
}

/// `[[Class alloc] init]`.
pub fn new(name: &CStr) -> Option<Obj> {
    let cls = class(name)?;
    unsafe { msg_send![cls, new] }
}

pub fn ns(s: &str) -> Retained<NSString> {
    NSString::from_str(s)
}

/// An `NSString` as Rust text; empty for nil.
pub fn string(obj: Option<&AnyObject>) -> String {
    let Some(obj) = obj else { return String::new() };
    let utf8: *const std::ffi::c_char = unsafe { msg_send![obj, UTF8String] };
    if utf8.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(utf8) }
        .to_string_lossy()
        .into_owned()
}

/// The items of an `NSArray` (or any object answering `count` / `objectAtIndex:`).
pub fn array(obj: Option<&AnyObject>) -> Vec<Obj> {
    let Some(obj) = obj else { return Vec::new() };
    let count: usize = unsafe { msg_send![obj, count] };
    (0..count)
        .filter_map(|i| unsafe { msg_send![obj, objectAtIndex: i] })
        .collect()
}

/// An `NSArray` of these objects.
pub fn nsarray(items: &[&AnyObject]) -> Obj {
    let cls = class(c"NSArray").expect("Foundation");
    let pointers: Vec<*const AnyObject> = items.iter().map(|o| *o as *const AnyObject).collect();
    unsafe { msg_send![cls, arrayWithObjects: pointers.as_ptr(), count: pointers.len()] }
}

/// `[NSNumber numberWithDouble:]`.
pub fn number(value: f64) -> Obj {
    let cls = class(c"NSNumber").expect("Foundation");
    unsafe { msg_send![cls, numberWithDouble: value] }
}

/// An `NSNumber`'s value; 0 for nil or anything else.
pub fn double(obj: Option<&AnyObject>) -> f64 {
    let Some(obj) = obj else { return 0. };
    let responds: bool = unsafe { msg_send![obj, respondsToSelector: objc2::sel!(doubleValue)] };
    if !responds {
        return 0.;
    }
    unsafe { msg_send![obj, doubleValue] }
}

/// `[dict objectForKey:key]`.
pub fn get(dict: &AnyObject, key: &str) -> Option<Obj> {
    let key = ns(key);
    unsafe { msg_send![dict, objectForKey: &*key] }
}

/// `localizedDescription` of an `NSError`.
pub fn error_text(error: *mut AnyObject) -> String {
    let Some(error) = (unsafe { error.as_ref() }) else {
        return "?".into();
    };
    let text: Option<Obj> = unsafe { msg_send![error, localizedDescription] };
    string(text.as_deref())
}

/// A C symbol of a loaded system framework, looked up at run time.
pub fn symbol(name: &CStr) -> Option<*mut std::ffi::c_void> {
    unsafe extern "C" {
        fn dlsym(
            handle: *mut std::ffi::c_void,
            symbol: *const std::ffi::c_char,
        ) -> *mut std::ffi::c_void;
    }
    // RTLD_DEFAULT
    let pointer = unsafe { dlsym(-2isize as *mut std::ffi::c_void, name.as_ptr()) };
    (!pointer.is_null()).then_some(pointer)
}

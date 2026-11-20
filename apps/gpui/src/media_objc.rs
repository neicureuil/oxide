//! Small helpers to call LiveKitWebRTC, AVFoundation and Foundation through the Objective-C runtime.
//! Objects are handled as `AnyObject`: the media code mirrors the Swift engine call for call, without bindings.
use objc2::rc::{Allocated, Retained};
use objc2::runtime::{AnyClass, AnyObject};
use objc2::{Encode, Encoding, msg_send};
use objc2_foundation::NSString;
use std::ffi::CStr;

pub type Obj = Retained<AnyObject>;

/// `CGSize`, as libwebrtc's renderers and AppKit take it.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CGSize {
    pub width: f64,
    pub height: f64,
}
unsafe impl Encode for CGSize {
    const ENCODING: Encoding = Encoding::Struct("CGSize", &[f64::ENCODING, f64::ENCODING]);
}

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
    CMFormatDescription = "opaqueCMFormatDescription",
    CMSampleBuffer = "opaqueCMSampleBuffer",
    AudioUnit = "ComponentInstanceRecord",
);

/// A class by runtime name; `None` when its framework is not loaded.
pub fn class(name: &CStr) -> Option<&'static AnyClass> {
    AnyClass::get(name)
}

/// LiveKitWebRTC was found at launch (it is linked weakly).
pub fn available() -> bool {
    class(c"LKRTCPeerConnectionFactory").is_some()
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

pub fn nsarray_of_strings(items: &[String]) -> Obj {
    let strings: Vec<Retained<NSString>> = items.iter().map(|s| ns(s)).collect();
    let refs: Vec<&AnyObject> = strings.iter().map(|s| -> &AnyObject { s }).collect();
    nsarray(&refs)
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

/// Whether `obj` is an instance of the named class (or a subclass).
pub fn is_kind(obj: &AnyObject, name: &CStr) -> bool {
    let Some(cls) = class(name) else { return false };
    unsafe { msg_send![obj, isKindOfClass: cls] }
}

/// Same object.
pub fn same(a: &AnyObject, b: &AnyObject) -> bool {
    std::ptr::eq(a, b)
}

/// `localizedDescription` of an `NSError`.
pub fn error_text(error: *mut AnyObject) -> String {
    let Some(error) = (unsafe { error.as_ref() }) else {
        return "?".into();
    };
    let text: Option<Obj> = unsafe { msg_send![error, localizedDescription] };
    string(text.as_deref())
}

/// A C function of the (weakly linked) framework, looked up at run time.
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

/// Appends a line to `~/Library/Logs/Oxide/calling.log`, like the macOS app's `callTrace`.
pub fn trace(message: &str) {
    use std::io::Write;
    let Some(home) = std::env::var_os("HOME") else {
        return;
    };
    let dir = std::path::Path::new(&home).join("Library/Logs/Oxide");
    let _ = std::fs::create_dir_all(&dir);
    let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join("calling.log"))
    else {
        return;
    };
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let _ = writeln!(file, "{now} media {message}");
}

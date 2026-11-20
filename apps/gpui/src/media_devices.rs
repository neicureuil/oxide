//! Microphones, outputs and cameras (`MediaDevices.swift`), the ones chosen in Settings (`DevicePreferences.swift`),
//! and camera / microphone permissions.
use crate::media_objc::{self, Obj, ns, string};
use crate::prefs::Prefs;
use block2::RcBlock;
use core_foundation::base::TCFType;
use core_foundation::string::{CFString, CFStringRef};
use objc2::msg_send;
use objc2::runtime::{AnyObject, Bool};
use std::ffi::{CStr, c_void};

pub type AudioDeviceId = u32;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AudioDevice {
    pub id: AudioDeviceId,
    /// Stable across reboots and reconnections: this is what preferences store.
    pub uid: String,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CameraDevice {
    pub unique_id: String,
    pub name: String,
}

// MARK: CoreAudio

#[repr(C)]
struct Address {
    selector: u32,
    scope: u32,
    element: u32,
}

const fn fourcc(code: &[u8; 4]) -> u32 {
    u32::from_be_bytes(*code)
}
const SYSTEM_OBJECT: u32 = 1;
const DEVICES: u32 = fourcc(b"dev#");
const DEFAULT_INPUT: u32 = fourcc(b"dIn ");
const DEFAULT_OUTPUT: u32 = fourcc(b"dOut");
const STREAMS: u32 = fourcc(b"stm#");
const DEVICE_UID: u32 = fourcc(b"uid ");
const OBJECT_NAME: u32 = fourcc(b"lnam");
const MUTE: u32 = fourcc(b"mute");
const SCOPE_GLOBAL: u32 = fourcc(b"glob");
const SCOPE_INPUT: u32 = fourcc(b"inpt");
const SCOPE_OUTPUT: u32 = fourcc(b"outp");
/// `kAudioOutputUnitProperty_CurrentDevice`.
const CURRENT_DEVICE: u32 = 2000;

type ListenerBlock = block2::Block<dyn Fn(u32, *const Address)>;

unsafe extern "C" {
    fn AudioObjectGetPropertyDataSize(
        id: u32,
        address: *const Address,
        qualifier_size: u32,
        qualifier: *const c_void,
        size: *mut u32,
    ) -> i32;
    fn AudioObjectGetPropertyData(
        id: u32,
        address: *const Address,
        qualifier_size: u32,
        qualifier: *const c_void,
        size: *mut u32,
        data: *mut c_void,
    ) -> i32;
    fn AudioObjectHasProperty(id: u32, address: *const Address) -> u8;
    fn AudioObjectAddPropertyListenerBlock(
        id: u32,
        address: *const Address,
        queue: *mut c_void,
        listener: &ListenerBlock,
    ) -> i32;
    fn AudioUnitSetProperty(
        unit: *mut c_void,
        id: u32,
        scope: u32,
        element: u32,
        data: *const c_void,
        size: u32,
    ) -> i32;
}

unsafe impl objc2::RefEncode for Address {
    const ENCODING_REF: objc2::Encoding = objc2::Encoding::Pointer(&objc2::Encoding::Struct(
        "AudioObjectPropertyAddress",
        &[
            objc2::Encoding::UInt,
            objc2::Encoding::UInt,
            objc2::Encoding::UInt,
        ],
    ));
}

fn address(selector: u32, scope: u32) -> Address {
    Address {
        selector,
        scope,
        element: 0,
    }
}

/// Thin wrappers over the CoreAudio HAL property API.
pub mod core_audio {
    use super::*;

    pub fn all() -> Vec<AudioDeviceId> {
        let address = address(DEVICES, SCOPE_GLOBAL);
        let mut size = 0u32;
        if unsafe {
            AudioObjectGetPropertyDataSize(SYSTEM_OBJECT, &address, 0, std::ptr::null(), &mut size)
        } != 0
        {
            return Vec::new();
        }
        let mut ids = vec![0u32; size as usize / 4];
        if unsafe {
            AudioObjectGetPropertyData(
                SYSTEM_OBJECT,
                &address,
                0,
                std::ptr::null(),
                &mut size,
                ids.as_mut_ptr().cast(),
            )
        } != 0
        {
            return Vec::new();
        }
        ids.truncate(size as usize / 4);
        ids
    }

    pub fn has_streams(id: AudioDeviceId, input: bool) -> bool {
        let address = address(STREAMS, if input { SCOPE_INPUT } else { SCOPE_OUTPUT });
        let mut size = 0u32;
        unsafe {
            AudioObjectGetPropertyDataSize(id, &address, 0, std::ptr::null(), &mut size) == 0
                && size > 0
        }
    }

    fn string(id: u32, selector: u32) -> Option<String> {
        let address = address(selector, SCOPE_GLOBAL);
        let mut value: CFStringRef = std::ptr::null();
        let mut size = std::mem::size_of::<CFStringRef>() as u32;
        let status = unsafe {
            AudioObjectGetPropertyData(
                id,
                &address,
                0,
                std::ptr::null(),
                &mut size,
                (&raw mut value).cast(),
            )
        };
        if status != 0 || value.is_null() {
            return None;
        }
        Some(unsafe { CFString::wrap_under_create_rule(value) }.to_string())
    }

    pub fn uid(id: AudioDeviceId) -> Option<String> {
        string(id, DEVICE_UID)
    }
    pub fn name(id: AudioDeviceId) -> Option<String> {
        string(id, OBJECT_NAME)
    }

    pub fn default_device(input: bool) -> Option<AudioDeviceId> {
        let address = address(
            if input { DEFAULT_INPUT } else { DEFAULT_OUTPUT },
            SCOPE_GLOBAL,
        );
        let mut id = 0u32;
        let mut size = 4u32;
        let status = unsafe {
            AudioObjectGetPropertyData(
                SYSTEM_OBJECT,
                &address,
                0,
                std::ptr::null(),
                &mut size,
                (&raw mut id).cast(),
            )
        };
        (status == 0 && id != 0).then_some(id)
    }

    /// Whether the device's output is muted (main element), when it reports it.
    pub fn is_muted(id: AudioDeviceId) -> bool {
        let address = address(MUTE, SCOPE_OUTPUT);
        if unsafe { AudioObjectHasProperty(id, &address) } == 0 {
            return false;
        }
        let mut muted = 0u32;
        let mut size = 4u32;
        unsafe {
            AudioObjectGetPropertyData(
                id,
                &address,
                0,
                std::ptr::null(),
                &mut size,
                (&raw mut muted).cast(),
            ) == 0
                && muted != 0
        }
    }

    pub fn device(uid: &str) -> Option<AudioDeviceId> {
        all()
            .into_iter()
            .find(|id| self::uid(*id).as_deref() == Some(uid))
    }

    pub fn devices(input: bool) -> Vec<AudioDevice> {
        let mut list: Vec<AudioDevice> = all()
            .into_iter()
            .filter(|id| has_streams(*id, input))
            .filter_map(|id| {
                let uid = uid(id)?;
                Some(AudioDevice {
                    id,
                    name: name(id).unwrap_or_else(|| uid.clone()),
                    uid,
                })
            })
            .collect();
        list.sort_by_key(|d| d.name.to_lowercase());
        list
    }

    /// Points the I/O unit of an `AVAudioEngine` node at a device. Must happen before the engine starts.
    pub fn use_device(device: AudioDeviceId, unit: *mut c_void) -> bool {
        if unit.is_null() {
            return false;
        }
        unsafe {
            AudioUnitSetProperty(unit, CURRENT_DEVICE, 0, 0, (&raw const device).cast(), 4) == 0
        }
    }

    /// Calls `change` (on a CoreAudio thread) whenever devices or system defaults change.
    pub fn observe(change: impl Fn() + Send + Sync + 'static) {
        let change = std::sync::Arc::new(change);
        for selector in [DEVICES, DEFAULT_INPUT, DEFAULT_OUTPUT] {
            let address = address(selector, SCOPE_GLOBAL);
            let change = change.clone();
            let block = RcBlock::new(move |_: u32, _: *const Address| change());
            unsafe {
                AudioObjectAddPropertyListenerBlock(
                    SYSTEM_OBJECT,
                    &address,
                    std::ptr::null_mut(),
                    &block,
                )
            };
            // Kept for the life of the app, like the listener.
            std::mem::forget(block);
        }
    }
}

// MARK: AVFoundation

/// An `NSString *const` exported by a system framework (`AVMediaTypeVideo`…).
pub fn constant(name: &CStr) -> Option<&'static AnyObject> {
    let pointer = media_objc::symbol(name)? as *const *const AnyObject;
    unsafe { (*pointer).as_ref() }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediaKind {
    Audio,
    Video,
}

impl MediaKind {
    pub fn av(self) -> Option<&'static AnyObject> {
        constant(match self {
            MediaKind::Audio => c"AVMediaTypeAudio",
            MediaKind::Video => c"AVMediaTypeVideo",
        })
    }
}

pub mod cameras {
    use super::*;

    pub fn all() -> Vec<Obj> {
        let Some(cls) = media_objc::class(c"AVCaptureDeviceDiscoverySession") else {
            return Vec::new();
        };
        let types: Vec<&AnyObject> = [
            c"AVCaptureDeviceTypeBuiltInWideAngleCamera",
            c"AVCaptureDeviceTypeExternal",
            c"AVCaptureDeviceTypeContinuityCamera",
        ]
        .into_iter()
        .filter_map(constant)
        .collect();
        let Some(video) = MediaKind::Video.av() else {
            return Vec::new();
        };
        let types = media_objc::nsarray(&types);
        let session: Option<Obj> = unsafe {
            msg_send![cls, discoverySessionWithDeviceTypes: &*types, mediaType: video, position: 0isize]
        };
        let Some(session) = session else {
            return Vec::new();
        };
        let devices: Option<Obj> = unsafe { msg_send![&*session, devices] };
        media_objc::array(devices.as_deref())
    }

    pub fn unique_id(device: &AnyObject) -> String {
        let id: Option<Obj> = unsafe { msg_send![device, uniqueID] };
        string(id.as_deref())
    }

    pub fn name(device: &AnyObject) -> String {
        let name: Option<Obj> = unsafe { msg_send![device, localizedName] };
        string(name.as_deref())
    }

    pub fn default_device() -> Option<Obj> {
        let cls = media_objc::class(c"AVCaptureDevice")?;
        let video = MediaKind::Video.av()?;
        unsafe { msg_send![cls, defaultDeviceWithMediaType: video] }
    }

    pub fn device(unique_id: &str) -> Option<Obj> {
        let cls = media_objc::class(c"AVCaptureDevice")?;
        let id = ns(unique_id);
        unsafe { msg_send![cls, deviceWithUniqueID: &*id] }
    }
}

/// Microphones, sound outputs and cameras currently connected.
#[derive(Clone, Debug, Default)]
pub struct Devices {
    pub microphones: Vec<AudioDevice>,
    pub speakers: Vec<AudioDevice>,
    pub cameras: Vec<CameraDevice>,
    pub default_microphone: Option<String>,
    pub default_speaker: Option<String>,
    pub default_camera: Option<String>,
}

impl Devices {
    pub fn load() -> Self {
        let mut devices = Self::default();
        devices.refresh();
        devices
    }

    pub fn refresh(&mut self) {
        self.microphones = core_audio::devices(true);
        self.speakers = core_audio::devices(false);
        self.cameras = cameras::all()
            .iter()
            .map(|d| CameraDevice {
                unique_id: cameras::unique_id(d),
                name: cameras::name(d),
            })
            .collect();
        self.default_microphone = core_audio::default_device(true).and_then(core_audio::name);
        self.default_speaker = core_audio::default_device(false).and_then(core_audio::name);
        self.default_camera = cameras::default_device().map(|d| cameras::name(&d));
    }
}

// MARK: Preferences

/// The preferred id when it is available, otherwise `None` (system default).
pub fn resolve<'a>(
    preferred: &'a str,
    available: impl IntoIterator<Item = &'a str>,
) -> Option<&'a str> {
    (!preferred.is_empty() && available.into_iter().any(|id| id == preferred)).then_some(preferred)
}

pub fn resolved_microphone(prefs: &Prefs) -> Option<AudioDeviceId> {
    let available = core_audio::devices(true);
    match resolve(&prefs.microphone, available.iter().map(|d| d.uid.as_str())) {
        Some(uid) => available.iter().find(|d| d.uid == uid).map(|d| d.id),
        None => core_audio::default_device(true),
    }
}

/// UID of the chosen output, or `None` to play on the system default.
pub fn resolved_speaker_uid(prefs: &Prefs) -> Option<String> {
    let available = core_audio::devices(false);
    resolve(&prefs.speaker, available.iter().map(|d| d.uid.as_str())).map(str::to_owned)
}

pub fn resolved_speaker(prefs: &Prefs) -> Option<AudioDeviceId> {
    resolved_speaker_uid(prefs)
        .and_then(|uid| core_audio::device(&uid))
        .or_else(|| core_audio::default_device(false))
}

pub fn resolved_camera(prefs: &Prefs) -> Option<Obj> {
    let all = cameras::all();
    let ids: Vec<String> = all.iter().map(|d| cameras::unique_id(d)).collect();
    if let Some(id) = resolve(&prefs.camera, ids.iter().map(String::as_str)) {
        return all.into_iter().find(|d| cameras::unique_id(d) == id);
    }
    cameras::default_device().or_else(|| all.into_iter().next())
}

// MARK: Permissions

/// Microphone and camera access, asked once and explained when refused.
pub mod permissions {
    use super::*;

    /// `AVAuthorizationStatus`: 0 not determined, 1 restricted, 2 denied, 3 authorized.
    pub fn status(kind: MediaKind) -> isize {
        let (Some(cls), Some(media)) = (media_objc::class(c"AVCaptureDevice"), kind.av()) else {
            return 2;
        };
        unsafe { msg_send![cls, authorizationStatusForMediaType: media] }
    }

    /// Granted, asking the system the first time.
    pub async fn request(kind: MediaKind) -> bool {
        match status(kind) {
            3 => return true,
            0 => {}
            _ => return false,
        }
        let (Some(cls), Some(media)) = (media_objc::class(c"AVCaptureDevice"), kind.av()) else {
            return false;
        };
        let (sender, receiver) = futures::channel::oneshot::channel::<bool>();
        let sender = std::sync::Mutex::new(Some(sender));
        let block = RcBlock::new(move |granted: Bool| {
            if let Some(sender) = sender.lock().unwrap().take() {
                let _ = sender.send(granted.as_bool());
            }
        });
        unsafe {
            let _: () =
                msg_send![cls, requestAccessForMediaType: media, completionHandler: &*block];
        }
        receiver.await.unwrap_or(false)
    }

    pub fn settings_url(kind: MediaKind) -> &'static str {
        match kind {
            MediaKind::Audio => {
                "x-apple.systempreferences:com.apple.preference.security?Privacy_Microphone"
            }
            MediaKind::Video => {
                "x-apple.systempreferences:com.apple.preference.security?Privacy_Camera"
            }
        }
    }

    unsafe extern "C" {
        fn CGPreflightScreenCaptureAccess() -> bool;
        fn CGRequestScreenCaptureAccess() -> bool;
    }

    /// Screen recording is allowed (asking once otherwise).
    pub fn screen_capture(ask: bool) -> bool {
        unsafe { CGPreflightScreenCaptureAccess() || (ask && CGRequestScreenCaptureAccess()) }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_falls_back_to_the_system_default() {
        assert_eq!(resolve("", ["a", "b"]), None);
        assert_eq!(resolve("c", ["a", "b"]), None);
        assert_eq!(resolve("b", ["a", "b"]), Some("b"));
    }

    #[test]
    fn core_audio_lists_devices_with_names() {
        // Every Mac has at least one output; names and UIDs come back as text.
        for device in core_audio::devices(false) {
            assert!(!device.uid.is_empty());
        }
    }
}

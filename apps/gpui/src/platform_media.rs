//! Sounds on the chosen speaker, voicemail playback (`VoicemailPlayer.swift`) and the Quick Look panel
//! (`QuickLook` in `FilesScreens.swift`).
use crate::platform::{Event, post};
use objc2::rc::Retained;
use objc2::runtime::{NSObjectProtocol, ProtocolObject};
use objc2::{
    AnyThread, DefinedClass, MainThreadMarker, MainThreadOnly, Message, define_class, msg_send,
};
use objc2_app_kit::{NSBeep, NSSound};
use objc2_avf_audio::{AVAudioPlayer, AVAudioPlayerDelegate};
use objc2_foundation::{NSData, NSInteger, NSObject, NSString, NSURL};
use objc2_quick_look_ui::{QLPreviewItem, QLPreviewPanel, QLPreviewPanelDataSource};
use std::cell::{Cell, RefCell};
use std::sync::Mutex;
use teams_core::NotificationSound;

/// UID of the output chosen in Settings; `None` plays on the system default.
static SPEAKER: Mutex<Option<String>> = Mutex::new(None);

/// Set by the device settings (CoreAudio's `kAudioDevicePropertyDeviceUID`), as `MediaDevicePreferences.resolvedSpeakerUID`.
pub fn set_speaker(uid: Option<String>) {
    *SPEAKER.lock().unwrap() = uid.filter(|u| !u.is_empty());
}

fn speaker() -> Option<Retained<NSString>> {
    SPEAKER.lock().unwrap().as_deref().map(NSString::from_str)
}

/// Teams sounds are not shipped with macOS: each gets the closest system sound.
pub fn system_sound_name(sound: NotificationSound) -> &'static str {
    match sound {
        NotificationSound::Default => "Glass",
        NotificationSound::Vibe => "Purr",
        NotificationSound::Nudge => "Pop",
        NotificationSound::Tap => "Tink",
        NotificationSound::Flick => "Morse",
        NotificationSound::Pluck => "Bottle",
        NotificationSound::Summon => "Hero",
        NotificationSound::Ping => "Ping",
        NotificationSound::DefaultUrgent => "Sosumi",
        NotificationSound::Prioritize => "Blow",
        NotificationSound::Escalate => "Funk",
        NotificationSound::Alarm => "Submarine",
    }
}

/// A system sound playing on the chosen speaker; stops when dropped.
pub struct SystemSound(Retained<NSSound>);

impl Drop for SystemSound {
    fn drop(&mut self) {
        self.0.stop();
    }
}

/// `name` is one of /System/Library/Sounds ("Submarine"…); `looped` for ringing.
pub fn play_system_sound(name: &str, looped: bool) -> Option<SystemSound> {
    let sound = NSSound::soundNamed(&NSString::from_str(name))?;
    sound.setLoops(looped);
    sound.setPlaybackDeviceIdentifier(speaker().as_deref());
    sound.play();
    Some(SystemSound(sound))
}

thread_local! {
    /// The latest banner's sound, kept until the next one.
    static BANNER_SOUND: RefCell<Option<SystemSound>> = const { RefCell::new(None) };
}

pub(crate) fn play_notification_sound(sound: NotificationSound) {
    let Some(made) = play_system_sound(system_sound_name(sound), false) else {
        NSBeep();
        return;
    };
    BANNER_SOUND.with(|s| *s.borrow_mut() = Some(made));
}

define_class!(
    /// Tells the window a voicemail ended, so its button shows "play" again.
    #[unsafe(super(NSObject))]
    #[name = "OxideVoicemailDelegate"]
    #[ivars = u64]
    struct VoicemailDelegate;

    unsafe impl NSObjectProtocol for VoicemailDelegate {}

    unsafe impl AVAudioPlayerDelegate for VoicemailDelegate {
        #[unsafe(method(audioPlayerDidFinishPlaying:successfully:))]
        fn did_finish(&self, _player: &AVAudioPlayer, _successfully: bool) {
            post(Event::VoicemailFinished(*self.ivars()));
        }
    }
);

impl VoicemailDelegate {
    fn new(generation: u64) -> Retained<Self> {
        let this = Self::alloc().set_ivars(generation);
        unsafe { msg_send![super(this), init] }
    }
}

struct Voicemail {
    id: String,
    generation: u64,
    player: Retained<AVAudioPlayer>,
    _delegate: Retained<VoicemailDelegate>,
}

thread_local! {
    static VOICEMAIL: RefCell<Option<Voicemail>> = const { RefCell::new(None) };
    static VOICEMAIL_ERROR: RefCell<Option<String>> = const { RefCell::new(None) };
    static GENERATION: Cell<u64> = const { Cell::new(0) };
}

pub(crate) fn play_voicemail(id: &str, data: Vec<u8>) {
    stop_voicemail();
    let generation = GENERATION.with(|g| {
        g.set(g.get() + 1);
        g.get()
    });
    let data = NSData::with_bytes(&data);
    let player = unsafe { AVAudioPlayer::initWithData_error(AVAudioPlayer::alloc(), &data) };
    let Ok(player) = player else {
        let text = crate::oxide::tr("ui.voicemail_player.couldn_t_play_the_voicemail");
        VOICEMAIL_ERROR.with(|e| *e.borrow_mut() = Some(text));
        return;
    };
    let delegate = VoicemailDelegate::new(generation);
    unsafe {
        player.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
        if let Some(uid) = speaker() {
            player.setCurrentDevice(Some(&uid));
        }
        player.play();
    }
    VOICEMAIL_ERROR.with(|e| *e.borrow_mut() = None);
    VOICEMAIL.with(|v| {
        *v.borrow_mut() = Some(Voicemail {
            id: id.to_owned(),
            generation,
            player,
            _delegate: delegate,
        })
    });
}

pub fn stop_voicemail() {
    if let Some(playing) = VOICEMAIL.with(|v| v.borrow_mut().take()) {
        unsafe { playing.player.stop() };
    }
}

/// The voicemail being played, for its play / stop button.
pub fn voicemail_playing() -> Option<String> {
    VOICEMAIL.with(|v| v.borrow().as_ref().map(|v| v.id.clone()))
}

/// Why the last voicemail could not be played.
pub fn voicemail_error() -> Option<String> {
    VOICEMAIL_ERROR.with(|e| e.borrow().clone())
}

pub(crate) fn voicemail_finished(generation: u64) {
    let current = VOICEMAIL.with(|v| v.borrow().as_ref().map(|v| v.generation));
    if current == Some(generation) {
        stop_voicemail();
    }
}

define_class!(
    /// Data source of the shared Quick Look panel, and its one item.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "OxideQuickLook"]
    #[ivars = RefCell<Option<Retained<NSURL>>>]
    struct QuickLook;

    unsafe impl NSObjectProtocol for QuickLook {}

    unsafe impl QLPreviewPanelDataSource for QuickLook {
        #[unsafe(method(numberOfPreviewItemsInPreviewPanel:))]
        fn count(&self, _panel: Option<&QLPreviewPanel>) -> NSInteger {
            self.ivars().borrow().is_some() as NSInteger
        }

        #[unsafe(method_id(previewPanel:previewItemAtIndex:))]
        fn item(
            &self,
            _panel: Option<&QLPreviewPanel>,
            _index: NSInteger,
        ) -> Option<Retained<ProtocolObject<dyn QLPreviewItem>>> {
            let this: Retained<Self> = self.retain();
            Some(ProtocolObject::from_retained(this))
        }
    }

    unsafe impl QLPreviewItem for QuickLook {
        #[unsafe(method_id(previewItemURL))]
        fn url(&self) -> Option<Retained<NSURL>> {
            self.ivars().borrow().clone()
        }
    }
);

thread_local! {
    static QUICK_LOOK: RefCell<Option<Retained<QuickLook>>> = const { RefCell::new(None) };
}

pub(crate) fn quick_look(path: &str) {
    let Some(mtm) = MainThreadMarker::new() else {
        return;
    };
    let source = QUICK_LOOK.with(|q| {
        q.borrow_mut()
            .get_or_insert_with(|| {
                let this = QuickLook::alloc(mtm).set_ivars(RefCell::new(None));
                unsafe { msg_send![super(this), init] }
            })
            .clone()
    });
    *source.ivars().borrow_mut() = Some(NSURL::fileURLWithPath(&NSString::from_str(path)));
    let Some(panel) = (unsafe { QLPreviewPanel::sharedPreviewPanel(mtm) }) else {
        // Should not happen; the command-line previewer still shows the file.
        let _ = std::process::Command::new("qlmanage")
            .args(["-p", path])
            .spawn();
        return;
    };
    unsafe {
        // Unretained by the panel: `QUICK_LOOK` keeps it.
        panel.setDataSource(Some(ProtocolObject::from_ref(&*source)));
        panel.reloadData();
    }
    panel.makeKeyAndOrderFront(None);
}

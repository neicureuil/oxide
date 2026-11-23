//! Microphones and speakers of the platform audio device module (CoreAudio, WASAPI, PulseAudio/ALSA).
use libwebrtc::peer_connection_factory::PeerConnectionFactory;
use libwebrtc::peer_connection_factory::native::PeerConnectionFactoryExt;

/// A device the user picked: matched by its platform id, else by name (ids can change across reboots).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceChoice {
    /// CoreAudio `AudioDeviceID`, WASAPI endpoint id, PulseAudio name.
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioDevice {
    pub id: String,
    pub name: String,
    /// A microphone, else a speaker.
    pub input: bool,
}

/// Microphones and speakers, defaults excluded; opens the audio device briefly, so not per frame.
pub fn audio_devices() -> Vec<AudioDevice> {
    let factory = PeerConnectionFactory::default();
    if !factory.acquire_platform_adm() {
        return Vec::new();
    }
    let devices = list(&factory);
    factory.release_platform_adm();
    devices
}

pub(crate) fn list(factory: &PeerConnectionFactory) -> Vec<AudioDevice> {
    let mut devices = Vec::new();
    for i in 0..factory.recording_devices().max(0) as u16 {
        devices.push(AudioDevice {
            id: factory.recording_device_guid(i),
            name: factory.recording_device_name(i),
            input: true,
        });
    }
    for i in 0..factory.playout_devices().max(0) as u16 {
        devices.push(AudioDevice {
            id: factory.playout_device_guid(i),
            name: factory.playout_device_name(i),
            input: false,
        });
    }
    // Index 0 is the system default's alias (id "default" on macOS); `None` already means it.
    devices.retain(|d| d.id != "default" && !d.name.starts_with("default ("));
    devices
}

/// Selects the chosen microphone and speaker; the defaults stay when they're unplugged.
pub(crate) fn apply(
    factory: &PeerConnectionFactory,
    microphone: Option<&DeviceChoice>,
    speaker: Option<&DeviceChoice>,
) {
    let devices = list(factory);
    let find = |choice: &DeviceChoice, input: bool| {
        let kind = || devices.iter().filter(move |d| d.input == input);
        kind()
            .find(|d| d.id == choice.id)
            .or_else(|| kind().find(|d| d.name == choice.name))
            .map(|d| d.id.clone())
    };
    if let Some(id) = microphone.and_then(|c| find(c, true)) {
        let ok = factory.set_recording_device_by_guid(&id);
        log::info!("microphone {id}: {ok}");
    }
    if let Some(id) = speaker.and_then(|c| find(c, false)) {
        let ok = factory.set_playout_device_by_guid(&id);
        log::info!("speaker {id}: {ok}");
    }
}

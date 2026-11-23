//! Rings an incoming call (`IncomingCallAlert.swift`): sound and Dock attention; and plays the ringback tone
//! while an outgoing call rings (`Ringback.swift`). Both on the output chosen in Settings.
use crate::prefs::Prefs;
use sound::Sound;
use teams_core::{CallPart, CallPhaseView};

/// The sounds, on the output chosen in Settings like the calls: `NSSound` on macOS. Elsewhere none yet (the call's
/// banner and window still show it).
#[cfg(target_os = "macos")]
mod sound {
    use crate::media_devices;
    use crate::objc_util::{self, Obj, ns};
    use crate::prefs::Prefs;
    use objc2::msg_send;

    pub struct Sound(Obj);

    impl Sound {
        /// A system alert sound by name.
        pub fn named(name: &str) -> Option<Self> {
            let cls = objc_util::class(c"NSSound")?;
            let sound: Option<Obj> = unsafe { msg_send![cls, soundNamed: &*ns(name)] };
            sound.map(Self)
        }

        /// A WAV file's bytes.
        pub fn wav(data: &[u8]) -> Option<Self> {
            let cls = objc_util::class(c"NSData")?;
            let data: Obj = unsafe {
                msg_send![cls, dataWithBytes: data.as_ptr().cast::<std::ffi::c_void>(), length: data.len()]
            };
            let sound: Option<Obj> =
                unsafe { msg_send![objc_util::alloc(c"NSSound")?, initWithData: &*data] };
            sound.map(Self)
        }

        /// Plays in a loop at `volume` on the chosen output, rather than the system default.
        pub fn play_looping(&self, volume: f32, prefs: &Prefs) {
            unsafe {
                let _: () = msg_send![&*self.0, setLoops: true];
                let _: () = msg_send![&*self.0, setVolume: volume];
                if let Some(uid) = media_devices::resolved_speaker_uid(prefs) {
                    let _: () = msg_send![&*self.0, setPlaybackDeviceIdentifier: &*ns(&uid)];
                }
                let _: bool = msg_send![&*self.0, play];
            }
        }
    }

    impl Drop for Sound {
        fn drop(&mut self) {
            unsafe {
                let _: bool = msg_send![&*self.0, stop];
            }
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod sound {
    use crate::prefs::Prefs;

    pub struct Sound;

    impl Sound {
        pub fn named(_name: &str) -> Option<Self> {
            None
        }
        pub fn wav(_data: &[u8]) -> Option<Self> {
            None
        }
        pub fn play_looping(&self, _volume: f32, _prefs: &Prefs) {}
    }
}

/// Rings an incoming call: sound and Dock attention.
#[derive(Default)]
pub struct IncomingAlert {
    ringing: Option<String>,
    sound: Option<Sound>,
    attention: Option<isize>,
}

impl IncomingAlert {
    pub fn sync(&mut self, part: Option<&CallPart>, call_sounds: bool, prefs: &Prefs) {
        let call = part
            .and_then(|p| p.active.as_ref())
            .filter(|c| c.phase == CallPhaseView::Incoming);
        if call.map(|c| &c.id) == self.ringing.as_ref() {
            return;
        }
        self.stop();
        let Some(call) = call else { return };
        self.ringing = Some(call.id.clone());
        if call_sounds && let Some(sound) = Sound::named("Submarine").or_else(|| Sound::named("Glass")) {
            sound.play_looping(1., prefs);
            self.sound = Some(sound);
        }
        // Bounces until the app is activated or the request cancelled.
        self.attention = crate::platform::request_attention(true);
        // In the background, a banner with Accept / Decline (`NotificationRouter.showCall`).
        if !crate::platform::is_active() {
            crate::platform::notify_call(&call.id, &call.title, call.offers_video);
        }
    }

    fn stop(&mut self) {
        self.sound = None;
        if let Some(attention) = self.attention.take() {
            crate::platform::cancel_attention(attention);
        }
        if let Some(id) = self.ringing.take() {
            crate::platform::withdraw_call(&id);
        }
    }
}

/// French ringback tone (440 Hz, 1.5 s on, 3.5 s off) while an outgoing call rings.
#[derive(Default)]
pub struct Ringback {
    ringing: Option<String>,
    sound: Option<Sound>,
}

impl Ringback {
    pub fn sync(&mut self, part: Option<&CallPart>, call_sounds: bool, prefs: &Prefs) {
        let call = part
            .and_then(|p| p.active.as_ref())
            .filter(|c| c.phase == CallPhaseView::Ringing && !c.is_meeting && !c.is_test);
        if call.map(|c| &c.id) == self.ringing.as_ref() {
            return;
        }
        self.sound = None;
        self.ringing = None;
        let Some(call) = call else { return };
        if !call_sounds {
            return;
        }
        self.ringing = Some(call.id.clone());
        let Some(sound) = Sound::wav(&tone()) else { return };
        sound.play_looping(0.35, prefs);
        self.sound = Some(sound);
    }
}

/// One 5 s period as 16-bit mono WAV.
fn tone() -> Vec<u8> {
    let rate = 16_000usize;
    let (on, total, fade) = (rate * 3 / 2, rate * 5, rate / 100);
    let mut samples = vec![0i16; total];
    for (i, sample) in samples.iter_mut().enumerate().take(on) {
        let envelope = (i.min(on - i) as f64 / fade as f64).min(1.);
        *sample = ((2. * std::f64::consts::PI * 440. * i as f64 / rate as f64).sin()
            * envelope
            * 12_000.) as i16;
    }
    let bytes = (total * 2) as u32;
    let mut data = Vec::with_capacity(44 + total * 2);
    data.extend_from_slice(b"RIFF");
    data.extend_from_slice(&(36 + bytes).to_le_bytes());
    data.extend_from_slice(b"WAVEfmt ");
    data.extend_from_slice(&16u32.to_le_bytes());
    data.extend_from_slice(&1u16.to_le_bytes());
    data.extend_from_slice(&1u16.to_le_bytes());
    data.extend_from_slice(&(rate as u32).to_le_bytes());
    data.extend_from_slice(&(rate as u32 * 2).to_le_bytes());
    data.extend_from_slice(&2u16.to_le_bytes());
    data.extend_from_slice(&16u16.to_le_bytes());
    data.extend_from_slice(b"data");
    data.extend_from_slice(&bytes.to_le_bytes());
    for sample in samples {
        data.extend_from_slice(&sample.to_le_bytes());
    }
    data
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ringback_is_a_five_second_wav() {
        let wav = tone();
        assert_eq!(&wav[..4], b"RIFF");
        assert_eq!(wav.len(), 44 + 16_000 * 5 * 2);
        // Silent after the 1.5 s tone.
        let at = |s: usize| i16::from_le_bytes([wav[44 + 2 * s], wav[45 + 2 * s]]);
        assert_eq!(at(16_000 * 2), 0);
        assert!((0..400).any(|s| at(s + 1000).abs() > 5_000));
    }
}

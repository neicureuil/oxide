//! Device checks outside calls (`DeviceTester.swift`): the chosen microphone's live level and a record-and-play-back
//! check, a chime on the chosen output, and a live camera preview.
use crate::media_devices::{self, MediaKind, core_audio, permissions};
use crate::media_engine::{MediaMsg, Window};
use crate::objc_util::{self, AudioUnit, CMSampleBuffer, CVBuffer, Obj};
use crate::media_video::FrameSlot;
use crate::oxide::{Oxide, tr};
use crate::prefs::Prefs;
use block2::RcBlock;
use futures::channel::mpsc::UnboundedSender;
use gpui::{Context, Task};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject};
use objc2::{AnyThread, DefinedClass, define_class, msg_send};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Which screen a tester belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Owner {
    Settings,
    Prejoin,
}

pub enum TesterMsg {
    Level(Owner, f32),
    PlaybackDone(Owner),
    SpeakerDone,
}

/// The meter is refreshed at most this often (~30 Hz), however small the audio buffers.
const PUBLISH_INTERVAL: Duration = Duration::from_millis(33);

#[derive(Default)]
struct TapState {
    peak: f32,
    last_publish: Option<Instant>,
    recording: bool,
    recorded: Vec<f32>,
}

/// Live input level of the chosen microphone, and a short record-and-play-back check.
pub struct MicrophoneTester {
    owner: Owner,
    /// 0…1, smoothed.
    pub level: f32,
    pub running: bool,
    pub recording: bool,
    pub playing_back: bool,
    pub error: Option<String>,
    engine: Option<Obj>,
    playback: Option<Obj>,
    rate: f64,
    tap: Arc<Mutex<TapState>>,
    echo: Option<Task<()>>,
}

fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.;
    }
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
}

/// -50 dBFS (silence) … 0 dBFS (full scale), with a fast attack and slow release.
fn smoothed(level: f32, rms: f32) -> f32 {
    let db = 20. * rms.max(1e-6).log10();
    let target = ((db + 50.) / 50.).clamp(0., 1.);
    if target > level {
        target
    } else {
        level * 0.85 + target * 0.15
    }
}

impl MicrophoneTester {
    pub fn new(owner: Owner) -> Self {
        Self {
            owner,
            level: 0.,
            running: false,
            recording: false,
            playing_back: false,
            error: None,
            engine: None,
            playback: None,
            rate: 48_000.,
            tap: Arc::default(),
            echo: None,
        }
    }

    fn open(&mut self, prefs: &Prefs, tx: UnboundedSender<MediaMsg>) {
        if self.running {
            return;
        }
        let Some(engine) = objc_util::new(c"AVAudioEngine") else {
            return;
        };
        let input: Obj = unsafe { msg_send![&*engine, inputNode] };
        if let Some(device) = media_devices::resolved_microphone(prefs) {
            let unit: *mut AudioUnit = unsafe { msg_send![&*input, audioUnit] };
            core_audio::use_device(device, unit.cast());
        }
        let format: Obj = unsafe { msg_send![&*input, inputFormatForBus: 0usize] };
        let channels: u32 = unsafe { msg_send![&*format, channelCount] };
        let rate: f64 = unsafe { msg_send![&*format, sampleRate] };
        if channels == 0 || rate <= 0. {
            self.error = Some(tr("ui.device_tester.this_microphone_isn_t_sending_any"));
            return;
        }
        self.rate = rate;
        *self.tap.lock().unwrap() = TapState::default();
        let tap = self.tap.clone();
        let owner = self.owner;
        let block = RcBlock::new(move |buffer: *mut AnyObject, _when: *mut AnyObject| {
            let Some(buffer) = (unsafe { buffer.as_ref() }) else {
                return;
            };
            let frames: u32 = unsafe { msg_send![buffer, frameLength] };
            let data: *const *mut f32 = unsafe { msg_send![buffer, floatChannelData] };
            if data.is_null() || frames == 0 || unsafe { (*data).is_null() } {
                return;
            }
            let samples = unsafe { std::slice::from_raw_parts(*data, frames as usize) };
            let level = rms(samples);
            let now = Instant::now();
            let peak = {
                let mut state = tap.lock().unwrap();
                if state.recording {
                    state.recorded.extend_from_slice(samples);
                }
                state.peak = state.peak.max(level);
                if state
                    .last_publish
                    .is_some_and(|last| now.duration_since(last) < PUBLISH_INTERVAL)
                {
                    None
                } else {
                    state.last_publish = Some(now);
                    Some(std::mem::take(&mut state.peak))
                }
            };
            if let Some(peak) = peak {
                let _ = tx.unbounded_send(MediaMsg::Tester(TesterMsg::Level(owner, peak)));
            }
        });
        unsafe {
            let _: () = msg_send![&*input, installTapOnBus: 0usize, bufferSize: 1024u32, format: &*format, block: &*block];
        }
        let mut error: *mut AnyObject = std::ptr::null_mut();
        let started: bool = unsafe { msg_send![&*engine, startAndReturnError: &mut error] };
        if started {
            self.engine = Some(engine);
            self.running = true;
            self.error = None;
        } else {
            unsafe {
                let _: () = msg_send![&*input, removeTapOnBus: 0usize];
            }
            self.error = Some(tr("ui.device_tester.couldn_t_open_the_microphone"));
        }
    }

    pub fn stop(&mut self) {
        if let Some(engine) = self.engine.take() {
            unsafe {
                let input: Obj = msg_send![&*engine, inputNode];
                let _: () = msg_send![&*input, removeTapOnBus: 0usize];
                let _: () = msg_send![&*engine, stop];
            }
        }
        if let Some(playback) = self.playback.take() {
            unsafe {
                let _: () = msg_send![&*playback, stop];
            }
        }
        self.echo = None;
        self.running = false;
        self.recording = false;
        self.playing_back = false;
        self.level = 0.;
        *self.tap.lock().unwrap() = TapState::default();
    }

    fn receive(&mut self, rms: f32) {
        if self.running {
            self.level = smoothed(self.level, rms);
        }
    }

    fn play(&mut self, samples: Vec<f32>, prefs: &Prefs, tx: UnboundedSender<MediaMsg>) {
        let owner = self.owner;
        match play_samples(&samples, self.rate, 1, prefs, move || {
            let _ = tx.unbounded_send(MediaMsg::Tester(TesterMsg::PlaybackDone(owner)));
        }) {
            Some(engine) => {
                self.playback = Some(engine);
                self.playing_back = true;
            }
            None => self.error = Some(tr("ui.device_tester.couldn_t_play_back_the_recording")),
        }
    }
}

/// Plays mono or stereo float samples on the chosen output; `done` runs when the last one played.
fn play_samples(
    samples: &[f32],
    rate: f64,
    channels: u32,
    prefs: &Prefs,
    done: impl Fn() + 'static,
) -> Option<Obj> {
    let frames = samples.len() / channels as usize;
    if frames == 0 {
        return None;
    }
    let format: Obj = unsafe {
        msg_send![objc_util::alloc(c"AVAudioFormat")?, initStandardFormatWithSampleRate: rate, channels: channels]
    };
    let buffer: Obj = unsafe {
        msg_send![objc_util::alloc(c"AVAudioPCMBuffer")?, initWithPCMFormat: &*format, frameCapacity: frames as u32]
    };
    unsafe {
        let _: () = msg_send![&*buffer, setFrameLength: frames as u32];
        let data: *const *mut f32 = msg_send![&*buffer, floatChannelData];
        if data.is_null() {
            return None;
        }
        for channel in 0..channels as usize {
            let target = std::slice::from_raw_parts_mut(*data.add(channel), frames);
            for (frame, t) in target.iter_mut().enumerate() {
                *t = samples[frame * channels as usize + channel];
            }
        }
    }
    let engine = objc_util::new(c"AVAudioEngine")?;
    let player = objc_util::new(c"AVAudioPlayerNode")?;
    unsafe {
        let _: () = msg_send![&*engine, attachNode: &*player];
        let mixer: Obj = msg_send![&*engine, mainMixerNode];
        let _: () = msg_send![&*engine, connect: &*player, to: &*mixer, format: &*format];
        if let Some(speaker) = media_devices::resolved_speaker(prefs) {
            let output: Obj = msg_send![&*engine, outputNode];
            let unit: *mut AudioUnit = msg_send![&*output, audioUnit];
            core_audio::use_device(speaker, unit.cast());
        }
        let mut error: *mut AnyObject = std::ptr::null_mut();
        let started: bool = msg_send![&*engine, startAndReturnError: &mut error];
        if !started {
            return None;
        }
        let done = RcBlock::new(done);
        let _: () = msg_send![&*player, scheduleBuffer: &*buffer, completionHandler: &*done];
        let _: () = msg_send![&*player, play];
    }
    Some(engine)
}

fn tester(this: &mut Oxide, owner: Owner) -> &mut MicrophoneTester {
    match owner {
        Owner::Settings => &mut this.ui.call.settings_mic,
        Owner::Prejoin => &mut this.ui.call.prejoin_mic,
    }
}

/// Starts the meter of `owner`'s tester, asking for the microphone first.
pub fn start_mic(this: &mut Oxide, owner: Owner, cx: &mut Context<Oxide>) {
    if tester(this, owner).running {
        return;
    }
    cx.spawn(async move |this, cx| {
        let granted = permissions::request(MediaKind::Audio).await;
        let _ = this.update(cx, |this, cx| {
            if !granted {
                tester(this, owner).error = Some(tr(
                    "ui.device_tester.microphone_access_is_denied_allow_oxide",
                ));
            } else {
                let prefs = this.prefs.clone();
                let tx = this.ui.call.media.tx.clone();
                tester(this, owner).open(&prefs, tx);
            }
            cx.notify();
            crate::call_window::redraw(this, cx);
        });
    })
    .detach();
}

pub fn stop_mic(this: &mut Oxide, owner: Owner) {
    tester(this, owner).stop();
}

/// Records five seconds, then plays them on the chosen output.
pub fn echo(this: &mut Oxide, owner: Owner, cx: &mut Context<Oxide>) {
    let t = tester(this, owner);
    if t.recording || t.playing_back {
        return;
    }
    if !t.running {
        start_mic(this, owner, cx);
    }
    let task = cx.spawn(async move |this, cx| {
        // The meter may still be opening (permission).
        for _ in 0..20 {
            let running = this
                .update(cx, |this, _| tester(this, owner).running)
                .unwrap_or(false);
            if running {
                break;
            }
            cx.background_executor()
                .timer(Duration::from_millis(100))
                .await;
        }
        let ok = this
            .update(cx, |this, cx| {
                let t = tester(this, owner);
                if !t.running {
                    return false;
                }
                let mut state = t.tap.lock().unwrap();
                state.recorded.clear();
                state.recording = true;
                drop(state);
                t.recording = true;
                cx.notify();
                true
            })
            .unwrap_or(false);
        if !ok {
            return;
        }
        cx.background_executor().timer(Duration::from_secs(5)).await;
        let _ = this.update(cx, |this, cx| {
            let prefs = this.prefs.clone();
            let tx = this.ui.call.media.tx.clone();
            let t = tester(this, owner);
            if !t.recording {
                return;
            }
            t.recording = false;
            let recorded = {
                let mut state = t.tap.lock().unwrap();
                state.recording = false;
                std::mem::take(&mut state.recorded)
            };
            t.play(recorded, &prefs, tx);
            cx.notify();
        });
    });
    tester(this, owner).echo = Some(task);
}

/// Plays a short two-tone chime on the chosen output.
#[derive(Default)]
pub struct SpeakerTester {
    pub playing: bool,
    pub error: Option<String>,
    engine: Option<Obj>,
}

fn chime(rate: f64) -> Vec<f32> {
    let frames = (rate * 1.2) as usize;
    let mut out = Vec::with_capacity(frames * 2);
    for i in 0..frames {
        let t = i as f64 / rate;
        let (frequency, start) = if t < 0.6 { (660., 0.) } else { (880., 0.6) };
        let local = t - start;
        let envelope = (local / 0.02).min(1.) * (-local * 4.).exp();
        let sample = (0.25 * envelope * (2. * std::f64::consts::PI * frequency * t).sin()) as f32;
        out.push(sample);
        out.push(sample);
    }
    out
}

pub fn play_chime(this: &mut Oxide) {
    if this.ui.call.speaker_test.playing {
        return;
    }
    let tx = this.ui.call.media.tx.clone();
    let engine = play_samples(&chime(44_100.), 44_100., 2, &this.prefs, move || {
        let _ = tx.unbounded_send(MediaMsg::Tester(TesterMsg::SpeakerDone));
    });
    let speaker = &mut this.ui.call.speaker_test;
    match engine {
        Some(engine) => {
            speaker.engine = Some(engine);
            speaker.playing = true;
            speaker.error = None;
        }
        None => speaker.error = Some(tr("ui.device_tester.couldn_t_use_this_audio_output")),
    }
}

pub fn on_msg(this: &mut Oxide, msg: TesterMsg, cx: &mut Context<Oxide>) {
    match msg {
        TesterMsg::Level(owner, rms) => tester(this, owner).receive(rms),
        TesterMsg::PlaybackDone(owner) => {
            let t = tester(this, owner);
            t.playing_back = false;
            if let Some(playback) = t.playback.take() {
                unsafe {
                    let _: () = msg_send![&*playback, stop];
                }
            }
        }
        TesterMsg::SpeakerDone => {
            let speaker = &mut this.ui.call.speaker_test;
            if let Some(engine) = speaker.engine.take() {
                unsafe {
                    let _: () = msg_send![&*engine, stop];
                }
            }
            speaker.playing = false;
        }
    }
    cx.notify();
    crate::call_window::redraw(this, cx);
}

// MARK: Camera preview

struct SendObj(Obj);
unsafe impl Send for SendObj {}
impl SendObj {
    fn take(self) -> Obj {
        self.0
    }
}

unsafe extern "C" {
    fn dispatch_queue_create(
        label: *const std::ffi::c_char,
        attributes: *mut std::ffi::c_void,
    ) -> *mut AnyObject;
    fn CMSampleBufferGetImageBuffer(buffer: *mut CMSampleBuffer) -> *mut CVBuffer;
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements; the slot is thread-safe.
    #[unsafe(super(NSObject))]
    #[name = "OxideCameraPreviewSink"]
    #[ivars = Arc<FrameSlot>]
    struct PreviewSink;

    impl PreviewSink {
        #[unsafe(method(captureOutput:didOutputSampleBuffer:fromConnection:))]
        fn output(&self, _output: *mut AnyObject, buffer: *mut CMSampleBuffer, _connection: *mut AnyObject) {
            if buffer.is_null() {
                return;
            }
            let pixels = unsafe { CMSampleBufferGetImageBuffer(buffer) };
            self.ivars().receive_pixels(pixels.cast());
        }
    }
);

/// Live preview of a camera, mirrored like a video call self-view.
pub struct CameraPreview {
    pub slot: Arc<FrameSlot>,
    session: Obj,
    _sink: Retained<PreviewSink>,
    _queue: Obj,
    current: Option<String>,
}

impl CameraPreview {
    pub fn new(window: Window, tx: UnboundedSender<MediaMsg>) -> Option<Self> {
        let session = objc_util::new(c"AVCaptureSession")?;
        let output = objc_util::new(c"AVCaptureVideoDataOutput")?;
        let slot = FrameSlot::new(true, window, tx);
        let sink = {
            let this = PreviewSink::alloc().set_ivars(slot.clone());
            let sink: Retained<PreviewSink> = unsafe { msg_send![super(this), init] };
            sink
        };
        let queue = unsafe {
            Retained::from_raw(dispatch_queue_create(
                c"camera-preview".as_ptr(),
                std::ptr::null_mut(),
            ))?
        };
        unsafe {
            // Full-range NV12, as the window draws it.
            let key =
                core_video::pixel_buffer::kCVPixelBufferPixelFormatTypeKey as *const AnyObject;
            let value = objc_util::number(f64::from(
                core_video::pixel_buffer::kCVPixelFormatType_420YpCbCr8BiPlanarFullRange,
            ));
            let dictionary = objc_util::class(c"NSDictionary")?;
            let settings: Obj = msg_send![dictionary, dictionaryWithObject: &*value, forKey: key];
            let _: () = msg_send![&*output, setVideoSettings: &*settings];
            let _: () = msg_send![&*output, setSampleBufferDelegate: &*sink, queue: &*queue];
            let can: bool = msg_send![&*session, canAddOutput: &*output];
            if can {
                let _: () = msg_send![&*session, addOutput: &*output];
            }
        }
        Some(Self {
            slot,
            session,
            _sink: sink,
            _queue: queue,
            current: None,
        })
    }

    /// Shows `device` (by unique id), switching inputs when it changes.
    pub fn show(&mut self, device: Option<Obj>) {
        let id = device.as_deref().map(media_devices::cameras::unique_id);
        if id == self.current {
            return;
        }
        self.current = id;
        let session = SendObj(self.session.clone());
        let device = device.map(SendObj);
        std::thread::spawn(move || {
            let session = session.take();
            let device = device.map(SendObj::take);
            unsafe {
                let _: () = msg_send![&*session, beginConfiguration];
                let inputs: Option<Obj> = msg_send![&*session, inputs];
                for input in objc_util::array(inputs.as_deref()) {
                    let _: () = msg_send![&*session, removeInput: &*input];
                }
                if let Some(device) = device {
                    let cls = objc_util::class(c"AVCaptureDeviceInput");
                    let mut error: *mut AnyObject = std::ptr::null_mut();
                    let input: Option<Obj> = cls.and_then(
                        |cls| msg_send![cls, deviceInputWithDevice: &*device, error: &mut error],
                    );
                    if let Some(input) = input {
                        let can: bool = msg_send![&*session, canAddInput: &*input];
                        if can {
                            let _: () = msg_send![&*session, addInput: &*input];
                        }
                    }
                }
                let _: () = msg_send![&*session, commitConfiguration];
                let running: bool = msg_send![&*session, isRunning];
                let inputs: Option<Obj> = msg_send![&*session, inputs];
                if !running && !objc_util::array(inputs.as_deref()).is_empty() {
                    let _: () = msg_send![&*session, startRunning];
                }
            }
        });
    }
}

impl Drop for CameraPreview {
    fn drop(&mut self) {
        let session = SendObj(self.session.clone());
        std::thread::spawn(move || {
            let session = session.take();
            unsafe {
                let _: () = msg_send![&*session, stopRunning];
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meter_rises_fast_and_falls_slowly() {
        let loud = smoothed(0., 0.5);
        assert!(loud > 0.8);
        let after = smoothed(loud, 0.);
        assert!(after > 0.6 && after < loud);
        assert_eq!(smoothed(0., 0.), 0.);
    }

    #[test]
    fn chime_is_stereo_and_fades() {
        let samples = chime(1_000.);
        assert_eq!(samples.len(), 2_400);
        assert_eq!(samples[100], samples[101]);
        assert!(samples.iter().all(|s| s.abs() <= 0.25));
    }
}

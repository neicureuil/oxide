//! Records the call's audio on this Mac only (not announced to the others): microphone and participants mixed
//! into a 48 kHz mono AAC `.m4a`, streamed to disk as it goes (`LocalRecorder.swift`).
//!
//! Inputs convert on their audio thread and add into a small ring on a common timeline (samples since the start);
//! a writer thread appends to the file what no input can still reach.
use crate::media_objc::{self, Obj, ns};
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject};
use objc2::{AnyThread, DefinedClass, define_class, msg_send};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

const SAMPLE_RATE: f64 = 48_000.;
/// Two seconds of mix: inputs add ahead, the writer takes what is older than `LATENCY`.
const CAPACITY: usize = 96_000;
const LATENCY: i64 = 14_400;
/// An input further than this from where the clock puts it jumps there (it paused, or its clock drifted).
const TOLERANCE: i64 = 4_800;

struct Mix {
    ring: Vec<f32>,
    /// Timeline frame of the ring's first unwritten sample.
    base: i64,
    /// Where each input has got to.
    inputs: HashMap<u64, i64>,
    stopped: bool,
}

struct Core {
    origin: Instant,
    mix: Mutex<Mix>,
    /// Tests drive the clock by hand.
    clock: Box<dyn Fn() -> f64 + Send + Sync>,
}

impl Core {
    fn now(&self) -> i64 {
        ((self.clock)() * SAMPLE_RATE).round() as i64
    }

    /// Adds samples that end now into the mix (audio thread).
    fn mix(&self, samples: &[f32], input: u64) {
        let end = self.now();
        let mut mix = self.mix.lock().unwrap();
        if mix.stopped {
            return;
        }
        let count = samples.len() as i64;
        let mut start = end - count;
        if let Some(cursor) = mix.inputs.get(&input)
            && (cursor - start).abs() <= TOLERANCE
        {
            start = *cursor;
        }
        mix.inputs.insert(input, start + count);
        // Too late for the writer, or too far ahead of it: drop those samples.
        let first = start.max(mix.base);
        let last = (start + count).min(mix.base + CAPACITY as i64);
        for frame in first..last.max(first) {
            let slot = (frame % CAPACITY as i64) as usize;
            mix.ring[slot] += samples[(frame - start) as usize];
        }
    }

    /// Takes the mix before `frame` out of the ring.
    fn take(&self, frame: i64) -> Vec<f32> {
        let mut mix = self.mix.lock().unwrap();
        let count = (frame - mix.base).clamp(0, CAPACITY as i64) as usize;
        let mut out = Vec::with_capacity(count);
        for i in 0..count {
            let slot = ((mix.base + i as i64) % CAPACITY as i64) as usize;
            out.push(mix.ring[slot].clamp(-1., 1.));
            mix.ring[slot] = 0.;
        }
        mix.base += count as i64;
        out
    }
}

/// The AAC file and a reusable buffer, used by one thread at a time.
struct FileWriter {
    file: Obj,
    chunk: Obj,
}
unsafe impl Send for FileWriter {}

impl FileWriter {
    fn open(path: &Path) -> Result<Self, String> {
        let keys = [
            ns("AVFormatIDKey"),
            ns("AVSampleRateKey"),
            ns("AVNumberOfChannelsKey"),
            ns("AVEncoderBitRateKey"),
        ];
        // kAudioFormatMPEG4AAC
        let values = [
            media_objc::number(u32::from_be_bytes(*b"aac ") as f64),
            media_objc::number(SAMPLE_RATE),
            media_objc::number(1.),
            media_objc::number(64_000.),
        ];
        let keys: Vec<&AnyObject> = keys.iter().map(|k| -> &AnyObject { k }).collect();
        let values: Vec<&AnyObject> = values.iter().map(|v| &**v).collect();
        let dictionary = media_objc::class(c"NSDictionary").ok_or("Foundation")?;
        let settings: Obj = unsafe {
            msg_send![dictionary, dictionaryWithObjects: values.as_ptr(), forKeys: keys.as_ptr(), count: keys.len()]
        };
        let url_class = media_objc::class(c"NSURL").ok_or("Foundation")?;
        let path_text = ns(&path.to_string_lossy());
        let url: Obj = unsafe { msg_send![url_class, fileURLWithPath: &*path_text] };
        let mut error: *mut AnyObject = std::ptr::null_mut();
        let file: Option<Obj> = unsafe {
            msg_send![
                media_objc::alloc(c"AVAudioFile").ok_or("AVFoundation")?,
                initForWriting: &*url,
                settings: &*settings,
                commonFormat: 1usize,
                interleaved: false,
                error: &mut error
            ]
        };
        let file = file.ok_or_else(|| media_objc::error_text(error))?;
        let format = mono_format().ok_or("AVAudioFormat")?;
        let chunk: Option<Obj> = unsafe {
            msg_send![
                media_objc::alloc(c"AVAudioPCMBuffer").ok_or("AVFoundation")?,
                initWithPCMFormat: &*format,
                frameCapacity: CAPACITY as u32
            ]
        };
        Ok(Self {
            file,
            chunk: chunk.ok_or("AVAudioPCMBuffer")?,
        })
    }

    fn write(&self, samples: &[f32]) {
        if samples.is_empty() {
            return;
        }
        unsafe {
            let channels: *const *mut f32 = msg_send![&*self.chunk, floatChannelData];
            if channels.is_null() || (*channels).is_null() {
                return;
            }
            std::ptr::copy_nonoverlapping(samples.as_ptr(), *channels, samples.len().min(CAPACITY));
            let _: () = msg_send![&*self.chunk, setFrameLength: samples.len().min(CAPACITY) as u32];
            let mut error: *mut AnyObject = std::ptr::null_mut();
            let ok: bool = msg_send![&*self.file, writeFromBuffer: &*self.chunk, error: &mut error];
            if !ok {
                media_objc::trace(&format!(
                    "recording write failed: {}",
                    media_objc::error_text(error)
                ));
            }
        }
    }
}

fn mono_format() -> Option<Obj> {
    unsafe {
        msg_send![
            media_objc::alloc(c"AVAudioFormat")?,
            initWithCommonFormat: 1usize,
            sampleRate: SAMPLE_RATE,
            channels: 1u32,
            interleaved: false
        ]
    }
}

pub struct LocalRecorder {
    pub path: PathBuf,
    pub started: SystemTime,
    core: Arc<Core>,
    file: Arc<Mutex<Option<FileWriter>>>,
    stop: Arc<AtomicBool>,
    writer: Option<std::thread::JoinHandle<()>>,
}

static NEXT_INPUT: AtomicU64 = AtomicU64::new(1);

impl LocalRecorder {
    pub fn new(path: PathBuf) -> Result<Self, String> {
        let origin = Instant::now();
        Self::with_clock(path, Box::new(move || origin.elapsed().as_secs_f64()))
    }

    fn with_clock(
        path: PathBuf,
        clock: Box<dyn Fn() -> f64 + Send + Sync>,
    ) -> Result<Self, String> {
        let file = FileWriter::open(&path)?;
        let core = Arc::new(Core {
            origin: Instant::now(),
            mix: Mutex::new(Mix {
                ring: vec![0.; CAPACITY],
                base: 0,
                inputs: HashMap::new(),
                stopped: false,
            }),
            clock,
        });
        let file = Arc::new(Mutex::new(Some(file)));
        let stop = Arc::new(AtomicBool::new(false));
        let writer = {
            let (core, file, stop) = (core.clone(), file.clone(), stop.clone());
            std::thread::Builder::new()
                .name("oxide.local-recorder".into())
                .spawn(move || {
                    while !stop.load(Ordering::Acquire) {
                        std::thread::sleep(Duration::from_millis(100));
                        let samples = core.take(core.now() - LATENCY);
                        if let Some(file) = file.lock().unwrap().as_ref() {
                            file.write(&samples);
                        }
                    }
                })
                .map_err(|e| e.to_string())?
        };
        let _ = core.origin;
        Ok(Self {
            path,
            started: SystemTime::now(),
            core,
            file,
            stop,
            writer: Some(writer),
        })
    }

    pub fn make_input(&self) -> Arc<Input> {
        Arc::new(Input {
            core: self.core.clone(),
            id: NEXT_INPUT.fetch_add(1, Ordering::Relaxed),
            resampler: Mutex::new(Resampler::default()),
        })
    }

    /// Writes the rest and closes the file.
    pub fn stop(mut self) -> PathBuf {
        self.finish();
        self.path.clone()
    }

    fn finish(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(writer) = self.writer.take() {
            let _ = writer.join();
        }
        let end = {
            let mut mix = self.core.mix.lock().unwrap();
            mix.stopped = true;
            mix.inputs
                .values()
                .copied()
                .max()
                .unwrap_or(0)
                .max(mix.base)
        };
        let samples = self.core.take(end.max(self.core.now()));
        // AVAudioFile finishes the file when released.
        if let Some(file) = self.file.lock().unwrap().take() {
            file.write(&samples);
        }
    }
}

impl Drop for LocalRecorder {
    fn drop(&mut self) {
        if self.writer.is_some() {
            self.finish();
        }
    }
}

/// `<folder>/<title> <yyyy-MM-dd HH.mm>.m4a`, never an existing file.
pub fn default_path(title: &str, folder: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(folder).map_err(|e| e.to_string())?;
    let stamp = chrono::Local::now().format("%Y-%m-%d %H.%M").to_string();
    let mut name: String = title
        .split(['/', ':', '\\'])
        .collect::<Vec<_>>()
        .join("-")
        .trim()
        .to_owned();
    if name.is_empty() {
        name = "Call".into();
    }
    let name: String = name.chars().take(120).collect();
    let stem = format!("{name} {stamp}");
    let mut path = folder.join(format!("{stem}.m4a"));
    let mut copy = 2;
    while path.exists() {
        path = folder.join(format!("{stem} {copy}.m4a"));
        copy += 1;
    }
    Ok(path)
}

pub fn default_folder() -> PathBuf {
    let home = std::env::var_os("HOME").unwrap_or_default();
    PathBuf::from(home).join("Documents/Oxide Recordings")
}

/// The folder chosen in Settings, else the default one.
pub fn folder(chosen: &str) -> PathBuf {
    if chosen.is_empty() {
        default_folder()
    } else {
        PathBuf::from(chosen)
    }
}

/// Linear resampling to 48 kHz, continuous across buffers.
#[derive(Default)]
struct Resampler {
    /// Position in the current buffer, in input samples, where index 0 is the previous buffer's last sample.
    position: f64,
    previous: f32,
    started: bool,
}

impl Resampler {
    fn run(&mut self, input: &[f32], rate: f64) -> Vec<f32> {
        if input.is_empty() {
            return Vec::new();
        }
        if (rate - SAMPLE_RATE).abs() < 1. {
            return input.to_vec();
        }
        if !self.started {
            self.started = true;
            self.previous = input[0];
            self.position = 1.;
        }
        let step = rate / SAMPLE_RATE;
        let n = input.len() as f64;
        let at = |k: usize| if k == 0 { self.previous } else { input[k - 1] };
        let mut out = Vec::with_capacity((n / step) as usize + 2);
        let mut position = self.position;
        while position < n {
            let index = position.floor() as usize;
            let fraction = (position - index as f64) as f32;
            out.push(at(index) * (1. - fraction) + at(index + 1) * fraction);
            position += step;
        }
        self.position = position - n;
        self.previous = input[input.len() - 1];
        out
    }
}

/// One source of the mix (a microphone, a remote track), fed from a single audio thread.
pub struct Input {
    core: Arc<Core>,
    id: u64,
    resampler: Mutex<Resampler>,
}

impl Input {
    /// Mono samples in -1…1 at `rate`.
    pub fn append(&self, samples: &[f32], rate: f64) {
        let out = self.resampler.lock().unwrap().run(samples, rate);
        if !out.is_empty() {
            self.core.mix(&out, self.id);
        }
    }

    /// A 10 ms block from the audio processing module (`LKRTCAudioBuffer`): floats in the 16-bit range, read only.
    fn append_processing_buffer(&self, buffer: &AnyObject) {
        let channels: usize = unsafe { msg_send![buffer, channels] };
        let frames: usize = unsafe { msg_send![buffer, frames] };
        if channels == 0 || frames == 0 {
            return;
        }
        let mut mono = vec![0f32; frames];
        for channel in 0..channels {
            let data: *mut f32 = unsafe { msg_send![buffer, rawBufferForChannel: channel] };
            if data.is_null() {
                continue;
            }
            let samples = unsafe { std::slice::from_raw_parts(data, frames) };
            for (m, s) in mono.iter_mut().zip(samples) {
                *m += s / 32_768. / channels as f32;
            }
        }
        self.append(&mono, frames as f64 * 100.);
    }

    /// An `AVAudioPCMBuffer` in float or 16-bit, interleaved or not, any channels.
    fn append_pcm(&self, buffer: &AnyObject) {
        let Some(mono) = pcm_to_mono(buffer) else {
            return;
        };
        self.append(&mono.0, mono.1);
    }
}

/// An `AVAudioPCMBuffer`'s samples, mixed down, and its sample rate.
fn pcm_to_mono(buffer: &AnyObject) -> Option<(Vec<f32>, f64)> {
    let frames: u32 = unsafe { msg_send![buffer, frameLength] };
    let format: Option<Obj> = unsafe { msg_send![buffer, format] };
    let format = format?;
    let rate: f64 = unsafe { msg_send![&*format, sampleRate] };
    let channels: u32 = unsafe { msg_send![&*format, channelCount] };
    let common: usize = unsafe { msg_send![&*format, commonFormat] };
    let interleaved: bool = unsafe { msg_send![&*format, isInterleaved] };
    let (frames, channels) = (frames as usize, channels as usize);
    if frames == 0 || channels == 0 {
        return None;
    }
    let mut mono = vec![0f32; frames];
    let scale = 1. / channels as f32;
    match common {
        // Float32
        1 => {
            let data: *const *mut f32 = unsafe { msg_send![buffer, floatChannelData] };
            mix_down(data, frames, channels, interleaved, scale, &mut mono, |s| s);
        }
        // Int16
        3 => {
            let data: *const *mut i16 = unsafe { msg_send![buffer, int16ChannelData] };
            mix_down(data, frames, channels, interleaved, scale, &mut mono, |s| {
                s as f32 / 32_768.
            });
        }
        _ => return None,
    }
    Some((mono, rate))
}

fn mix_down<T: Copy>(
    data: *const *mut T,
    frames: usize,
    channels: usize,
    interleaved: bool,
    scale: f32,
    mono: &mut [f32],
    value: impl Fn(T) -> f32,
) {
    if data.is_null() {
        return;
    }
    unsafe {
        if interleaved {
            let samples = *data;
            if samples.is_null() {
                return;
            }
            let samples = std::slice::from_raw_parts(samples, frames * channels);
            for (frame, m) in mono.iter_mut().enumerate() {
                for channel in 0..channels {
                    *m += value(samples[frame * channels + channel]) * scale;
                }
            }
        } else {
            for channel in 0..channels {
                let samples = *data.add(channel);
                if samples.is_null() {
                    continue;
                }
                for (m, s) in mono
                    .iter_mut()
                    .zip(std::slice::from_raw_parts(samples, frames))
                {
                    *m += value(*s) * scale;
                }
            }
        }
    }
}

pub struct TapState {
    input: Option<Arc<Input>>,
    muted: bool,
}

define_class!(
    /// The microphone after libwebrtc's processing: a passthrough that copies into the recording, if any, while unmuted.
    // SAFETY: NSObject has no subclassing requirements; the ivars are behind a lock.
    #[unsafe(super(NSObject))]
    #[name = "OxideMicrophoneTap"]
    #[ivars = Mutex<TapState>]
    pub struct MicrophoneTap;

    impl MicrophoneTap {
        #[unsafe(method(audioProcessingInitializeWithSampleRate:channels:))]
        fn initialize(&self, _rate: usize, _channels: usize) {}

        #[unsafe(method(audioProcessingProcess:))]
        fn process(&self, buffer: *mut AnyObject) {
            let input = {
                let state = self.ivars().lock().unwrap();
                if state.muted { None } else { state.input.clone() }
            };
            if let (Some(input), Some(buffer)) = (input, unsafe { buffer.as_ref() }) {
                input.append_processing_buffer(buffer);
            }
        }

        #[unsafe(method(audioProcessingRelease))]
        fn release(&self) {}
    }
);

impl MicrophoneTap {
    pub fn new() -> Retained<Self> {
        let this = Self::alloc().set_ivars(Mutex::new(TapState {
            input: None,
            muted: false,
        }));
        unsafe { msg_send![super(this), init] }
    }
    pub fn set_muted(&self, muted: bool) {
        self.ivars().lock().unwrap().muted = muted;
    }
    pub fn set_input(&self, input: Option<Arc<Input>>) {
        self.ivars().lock().unwrap().input = input;
    }
}

define_class!(
    /// A participants' track into the recording.
    // SAFETY: NSObject has no subclassing requirements; the input is thread-safe.
    #[unsafe(super(NSObject))]
    #[name = "OxideRemoteAudioTap"]
    #[ivars = Arc<Input>]
    pub struct RemoteAudioTap;

    impl RemoteAudioTap {
        #[unsafe(method(renderPCMBuffer:))]
        fn render(&self, buffer: *mut AnyObject) {
            if let Some(buffer) = unsafe { buffer.as_ref() } {
                self.ivars().append_pcm(buffer);
            }
        }
    }
);

impl RemoteAudioTap {
    pub fn new(input: Arc<Input>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(input);
        unsafe { msg_send![super(this), init] }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resampler_keeps_the_rate() {
        let mut r = Resampler::default();
        let out: usize = (0..100).map(|_| r.run(&[0.5; 160], 16_000.).len()).sum();
        // 100 × 10 ms at 16 kHz: one second at 48 kHz, give or take a sample.
        assert!((47_990..=48_010).contains(&out), "{out}");
        assert_eq!(
            Resampler::default().run(&[0.25; 480], 48_000.),
            vec![0.25; 480]
        );
    }

    #[test]
    fn default_path_never_overwrites() {
        let dir = std::env::temp_dir().join(format!("oxide-rec-{}", std::process::id()));
        let first = default_path("Daily: team/sync", &dir).unwrap();
        assert!(
            first
                .file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("Daily- team-sync ")
        );
        std::fs::write(&first, b"x").unwrap();
        let second = default_path("Daily: team/sync", &dir).unwrap();
        assert_ne!(first, second);
        assert!(second.to_string_lossy().ends_with(" 2.m4a"));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn records_two_inputs_into_an_m4a() {
        let dir = std::env::temp_dir().join(format!("oxide-rec-file-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.m4a");
        let clock = Arc::new(Mutex::new(0f64));
        let read = clock.clone();
        let recorder =
            LocalRecorder::with_clock(path.clone(), Box::new(move || *read.lock().unwrap()))
                .unwrap();
        let (a, b) = (recorder.make_input(), recorder.make_input());
        for _ in 0..50 {
            *clock.lock().unwrap() += 0.01;
            a.append(&[0.1; 480], 48_000.);
            b.append(&[0.1; 160], 16_000.);
        }
        let out = recorder.stop();
        assert!(std::fs::metadata(&out).unwrap().len() > 0);
        let _ = std::fs::remove_dir_all(dir);
    }
}

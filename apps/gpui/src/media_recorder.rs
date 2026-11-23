//! Records the call's audio on this computer only (not announced to the others): microphone and participants mixed
//! into a 48 kHz mono file streamed to disk as it goes (`LocalRecorder.swift`): AAC `.m4a` on macOS, `.wav` elsewhere.
//! The engine hands the audio over as 16-bit PCM (`PcmTap`).
//!
//! Inputs convert on their audio thread and add into a small ring on a common timeline (samples since the start);
//! a writer thread appends to the file what no input can still reach.
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

#[cfg(target_os = "macos")]
use mac::FileWriter;
#[cfg(not(target_os = "macos"))]
use wav::FileWriter;

/// The recording's extension.
#[cfg(target_os = "macos")]
const EXTENSION: &str = "m4a";
#[cfg(not(target_os = "macos"))]
const EXTENSION: &str = "wav";

#[cfg(target_os = "macos")]
mod mac {
    use super::{CAPACITY, SAMPLE_RATE};
    use crate::objc_util::{self, Obj, ns};
    use objc2::msg_send;
    use objc2::runtime::AnyObject;
    use std::path::Path;

    /// The AAC file and a reusable buffer, used by one thread at a time.
    pub struct FileWriter {
        file: Obj,
        chunk: Obj,
    }
    unsafe impl Send for FileWriter {}

    impl FileWriter {
        pub fn open(path: &Path) -> Result<Self, String> {
            let keys = [
                ns("AVFormatIDKey"),
                ns("AVSampleRateKey"),
                ns("AVNumberOfChannelsKey"),
                ns("AVEncoderBitRateKey"),
            ];
            // kAudioFormatMPEG4AAC
            let values = [
                objc_util::number(u32::from_be_bytes(*b"aac ") as f64),
                objc_util::number(SAMPLE_RATE),
                objc_util::number(1.),
                objc_util::number(64_000.),
            ];
            let keys: Vec<&AnyObject> = keys.iter().map(|k| -> &AnyObject { k }).collect();
            let values: Vec<&AnyObject> = values.iter().map(|v| &**v).collect();
            let dictionary = objc_util::class(c"NSDictionary").ok_or("Foundation")?;
            let settings: Obj = unsafe {
                msg_send![dictionary, dictionaryWithObjects: values.as_ptr(), forKeys: keys.as_ptr(), count: keys.len()]
            };
            let url_class = objc_util::class(c"NSURL").ok_or("Foundation")?;
            let path_text = ns(&path.to_string_lossy());
            let url: Obj = unsafe { msg_send![url_class, fileURLWithPath: &*path_text] };
            let mut error: *mut AnyObject = std::ptr::null_mut();
            let file: Option<Obj> = unsafe {
                msg_send![
                    objc_util::alloc(c"AVAudioFile").ok_or("AVFoundation")?,
                    initForWriting: &*url,
                    settings: &*settings,
                    commonFormat: 1usize,
                    interleaved: false,
                    error: &mut error
                ]
            };
            let file = file.ok_or_else(|| objc_util::error_text(error))?;
            let format = mono_format().ok_or("AVAudioFormat")?;
            let chunk: Option<Obj> = unsafe {
                msg_send![
                    objc_util::alloc(c"AVAudioPCMBuffer").ok_or("AVFoundation")?,
                    initWithPCMFormat: &*format,
                    frameCapacity: CAPACITY as u32
                ]
            };
            Ok(Self {
                file,
                chunk: chunk.ok_or("AVAudioPCMBuffer")?,
            })
        }

        pub fn write(&self, samples: &[f32]) {
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
                    crate::media_engine::trace(&format!(
                        "recording write failed: {}",
                        objc_util::error_text(error)
                    ));
                }
            }
        }
    }

    fn mono_format() -> Option<Obj> {
        unsafe {
            msg_send![
                objc_util::alloc(c"AVAudioFormat")?,
                initWithCommonFormat: 1usize,
                sampleRate: SAMPLE_RATE,
                channels: 1u32,
                interleaved: false
            ]
        }
    }

}

/// 16-bit mono WAV, its sizes written when it closes.
#[cfg(not(target_os = "macos"))]
mod wav {
    use super::SAMPLE_RATE;
    use std::fs::File;
    use std::io::{BufWriter, Seek, SeekFrom, Write};
    use std::path::Path;
    use std::sync::Mutex;

    pub struct FileWriter {
        file: Mutex<BufWriter<File>>,
        bytes: Mutex<u32>,
    }

    fn header(data: u32) -> [u8; 44] {
        let rate = SAMPLE_RATE as u32;
        let mut h = [0u8; 44];
        h[0..4].copy_from_slice(b"RIFF");
        h[4..8].copy_from_slice(&(36 + data).to_le_bytes());
        h[8..16].copy_from_slice(b"WAVEfmt ");
        h[16..20].copy_from_slice(&16u32.to_le_bytes());
        h[20..22].copy_from_slice(&1u16.to_le_bytes());
        h[22..24].copy_from_slice(&1u16.to_le_bytes());
        h[24..28].copy_from_slice(&rate.to_le_bytes());
        h[28..32].copy_from_slice(&(rate * 2).to_le_bytes());
        h[32..34].copy_from_slice(&2u16.to_le_bytes());
        h[34..36].copy_from_slice(&16u16.to_le_bytes());
        h[36..40].copy_from_slice(b"data");
        h[40..44].copy_from_slice(&data.to_le_bytes());
        h
    }

    impl FileWriter {
        pub fn open(path: &Path) -> Result<Self, String> {
            let mut file = BufWriter::new(File::create(path).map_err(|e| e.to_string())?);
            file.write_all(&header(0)).map_err(|e| e.to_string())?;
            Ok(Self {
                file: Mutex::new(file),
                bytes: Mutex::new(0),
            })
        }

        pub fn write(&self, samples: &[f32]) {
            let mut file = self.file.lock().unwrap();
            let mut bytes = self.bytes.lock().unwrap();
            for s in samples {
                let v = (s.clamp(-1., 1.) * 32_767.) as i16;
                if file.write_all(&v.to_le_bytes()).is_ok() {
                    *bytes = bytes.saturating_add(2);
                }
            }
        }
    }

    impl Drop for FileWriter {
        fn drop(&mut self) {
            let bytes = *self.bytes.lock().unwrap();
            let file = self.file.get_mut().unwrap();
            let _ = file.seek(SeekFrom::Start(0));
            let _ = file.write_all(&header(bytes));
            let _ = file.flush();
        }
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
        // The file is finished when released.
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

/// `<folder>/<title> <yyyy-MM-dd HH.mm>.m4a` (`.wav` off macOS), never an existing file.
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
    let mut path = folder.join(format!("{stem}.{EXTENSION}"));
    let mut copy = 2;
    while path.exists() {
        path = folder.join(format!("{stem} {copy}.{EXTENSION}"));
        copy += 1;
    }
    Ok(path)
}

pub fn default_folder() -> PathBuf {
    crate::paths::home()
        .unwrap_or_default()
        .join("Documents")
        .join("Oxide Recordings")
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
}

/// The engine's PCM (`teams_media::PcmSink`) into the recording: our microphone (the engine holds it back while
/// muted) and the participants, each on its own input of the mix.
pub struct PcmTap {
    inputs: Mutex<Option<(Arc<Input>, Arc<Input>)>>,
}

impl PcmTap {
    pub fn new(recorder: &LocalRecorder) -> Arc<Self> {
        Arc::new(Self {
            inputs: Mutex::new(Some((recorder.make_input(), recorder.make_input()))),
        })
    }

    /// Lets go of the recording: late callbacks write nothing.
    pub fn stop(&self) {
        *self.inputs.lock().unwrap() = None;
    }
}

impl teams_media::PcmSink for PcmTap {
    fn on_pcm(&self, remote: bool, samples: &[i16], sample_rate: u32, channels: u32) {
        let input = match self.inputs.lock().unwrap().as_ref() {
            Some((microphone, participants)) => {
                if remote { participants.clone() } else { microphone.clone() }
            }
            None => return,
        };
        let channels = channels as usize;
        if channels == 0 || samples.is_empty() {
            return;
        }
        input.append(&to_mono(samples, channels), f64::from(sample_rate));
    }
}

/// Interleaved 16-bit frames mixed down to mono floats in -1…1.
fn to_mono(samples: &[i16], channels: usize) -> Vec<f32> {
    let scale = 1. / (32_768. * channels as f32);
    samples
        .chunks_exact(channels)
        .map(|frame| frame.iter().map(|s| f32::from(*s)).sum::<f32>() * scale)
        .collect()
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
        assert!(second.to_string_lossy().ends_with(&format!(" 2.{EXTENSION}")));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn mono_mixes_interleaved_channels() {
        assert_eq!(to_mono(&[16_384, -16_384, 8_192, 8_192], 2), vec![0., 0.25]);
        assert_eq!(to_mono(&[-32_768], 1), vec![-1.]);
    }

    #[test]
    fn records_two_inputs_into_a_file() {
        let dir = std::env::temp_dir().join(format!("oxide-rec-file-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(format!("test.{EXTENSION}"));
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

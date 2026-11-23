//! Teams fork: the microphone after echo cancellation and noise suppression, for local recordings.
use std::sync::{Arc, RwLock};

/// Receives 10 ms blocks of 16-bit interleaved samples: (samples, sample rate, channels).
pub type MicTapSink = Arc<dyn Fn(&[i16], u32, u32) + Send + Sync>;

static SINK: RwLock<Option<MicTapSink>> = RwLock::new(None);

/// Forwards the processed microphone to `sink`, or stops. One sink for the process: one call at a time.
pub fn set_mic_tap(sink: Option<MicTapSink>) {
    let enabled = sink.is_some();
    *SINK.write().unwrap() = sink;
    ffi::set_mic_tap_enabled(enabled);
}

fn on_mic_pcm(samples: &[i16], sample_rate: u32, channels: u32) {
    if let Some(sink) = SINK.read().unwrap().as_ref() {
        sink(samples, sample_rate, channels);
    }
}

#[cxx::bridge(namespace = "livekit_ffi")]
pub mod ffi {
    unsafe extern "C++" {
        include!("livekit/mic_tap.h");

        fn set_mic_tap_enabled(enabled: bool);
    }

    extern "Rust" {
        fn on_mic_pcm(samples: &[i16], sample_rate: u32, channels: u32);
    }
}

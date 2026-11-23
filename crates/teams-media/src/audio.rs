//! PCM taps for a local recording: the microphone after processing, and each participant's audio.
use std::sync::Arc;

use libwebrtc::audio_frame::AudioFrame;
use libwebrtc::audio_source::AudioSourceOptions;
use libwebrtc::audio_source::native::NativeAudioSource;
use libwebrtc::audio_stream::native::NativeAudioStream;
use libwebrtc::peer_connection_factory::PeerConnectionFactory;
use libwebrtc::peer_connection_factory::native::PeerConnectionFactoryExt;
use libwebrtc::prelude::RtcAudioTrack;
use tokio_stream::StreamExt;

use crate::PcmSink;

/// Rate the participants' audio is resampled to for the sink.
const REMOTE_RATE: i32 = 48_000;

pub(crate) fn set_mic_tap(sink: Option<Arc<dyn PcmSink>>) {
    libwebrtc::native::set_mic_tap(sink.map(|sink| {
        Arc::new(move |samples: &[i16], rate, channels| sink.on_pcm(false, samples, rate, channels))
            as libwebrtc::native::MicTapSink
    }));
}

/// Forwards a participant's audio to `sink` until the track ends or the task is aborted.
pub(crate) fn pump_remote(
    track: RtcAudioTrack,
    sink: Arc<dyn PcmSink>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut stream = NativeAudioStream::new(track, REMOTE_RATE, 1);
        while let Some(frame) = stream.next().await {
            sink.on_pcm(true, &frame.data, frame.sample_rate, frame.num_channels);
        }
    })
}

/// A 440 Hz tone as microphone, for engines without audio device; the task feeds it until aborted.
pub(crate) fn tone_track(
    factory: &PeerConnectionFactory,
) -> (RtcAudioTrack, tokio::task::JoinHandle<()>) {
    const RATE: u32 = 48_000;
    const BLOCK: u32 = RATE / 100;
    let source = NativeAudioSource::new(AudioSourceOptions::default(), RATE, 1, 100);
    let track = factory.create_audio_track("audio0", source.clone());
    let task = tokio::spawn(async move {
        let mut ticks = tokio::time::interval(std::time::Duration::from_millis(10));
        let mut phase = 0u32;
        loop {
            ticks.tick().await;
            let mut frame = AudioFrame::new(RATE, 1, BLOCK);
            for sample in frame.data.to_mut().iter_mut() {
                let t = f64::from(phase) / f64::from(RATE);
                *sample = ((t * 440.0 * std::f64::consts::TAU).sin() * 3000.0) as i16;
                phase = (phase + 1) % RATE;
            }
            let _ = source.capture_frame(&frame).await;
        }
    });
    (track, task)
}

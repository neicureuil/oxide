//! What the engine saw during a call, from libwebrtc's statistics: the Echo Bot test report's inputs.
use libwebrtc::stats::RtcStats;

#[derive(Debug, Clone, Default, PartialEq)]
pub struct CallDiagnostics {
    pub camera_requested: bool,
    pub camera_failed: bool,
    /// Media reached the connected state at least once.
    pub connected: bool,
    pub disconnects: u32,
    pub failed: bool,
    pub failure_reason: Option<String>,
    /// `totalAudioEnergy` of the microphone source: stays ~0 when nothing is captured.
    pub mic_energy: f64,
    pub audio_packets_received: u64,
    pub audio_packets_lost: u64,
    /// Worst round-trip time reported by the other side, in seconds.
    pub round_trip: f64,
    pub video_frames_sent: u64,
}

impl CallDiagnostics {
    pub(crate) fn merge(&mut self, sample: &Sample) {
        self.mic_energy = self.mic_energy.max(sample.mic_energy);
        self.audio_packets_received = self
            .audio_packets_received
            .max(sample.audio_packets_received);
        self.audio_packets_lost = self.audio_packets_lost.max(sample.audio_packets_lost);
        self.round_trip = self.round_trip.max(sample.round_trip);
        self.video_frames_sent = self.video_frames_sent.max(sample.video_frames_sent);
    }
}

/// One `getStats` report, summed over streams.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Sample {
    pub mic_energy: f64,
    /// Microphone level now, 0…1.
    pub mic_level: f64,
    pub audio_packets_received: u64,
    pub audio_packets_lost: u64,
    pub round_trip: f64,
    pub video_frames_sent: u64,
}

pub(crate) fn sample(report: &[RtcStats]) -> Sample {
    let mut sample = Sample::default();
    for stat in report {
        match stat {
            RtcStats::MediaSource(s) if s.source.kind == "audio" => {
                sample.mic_energy = sample.mic_energy.max(s.audio.total_audio_energy);
                sample.mic_level = sample.mic_level.max(s.audio.audio_level);
            }
            RtcStats::InboundRtp(s) if s.stream.kind == "audio" => {
                sample.audio_packets_received += s.received.packets_received;
                sample.audio_packets_lost += s.received.packets_lost.max(0) as u64;
            }
            RtcStats::RemoteInboundRtp(s) => {
                sample.round_trip = sample.round_trip.max(s.remote_inbound.round_trip_time);
            }
            RtcStats::OutboundRtp(s) if s.stream.kind == "video" => {
                sample.video_frames_sent += u64::from(s.outbound.frames_encoded);
            }
            _ => {}
        }
    }
    sample
}

/// Each video stream in one line, for the call log: what goes out, what comes in, and why it stalls.
pub(crate) fn video_summary(report: &[RtcStats]) -> String {
    let mut parts = Vec::new();
    for stat in report {
        match stat {
            RtcStats::OutboundRtp(s) if s.stream.kind == "video" => parts.push(format!(
                "out[{}] encoded={} sent={} key={} {}px {:.0}fps limit={:?} pli={} enc={}",
                s.stream.ssrc,
                s.outbound.frames_encoded,
                s.outbound.frames_sent,
                s.outbound.key_frames_encoded,
                s.outbound.frame_width,
                s.outbound.frames_per_second,
                s.outbound.quality_limitation_reason,
                s.outbound.pli_count,
                s.outbound.encoder_implementation,
            )),
            RtcStats::InboundRtp(s) if s.stream.kind == "video" => parts.push(format!(
                "in[{}] received={} decoded={} key={} dropped={} {}px pli={} dec={}",
                s.stream.ssrc,
                s.inbound.frames_received,
                s.inbound.frames_decoded,
                s.inbound.key_frames_decoded,
                s.inbound.frames_dropped,
                s.inbound.frame_width,
                s.inbound.pli_count,
                s.inbound.decoder_implementation,
            )),
            _ => {}
        }
    }
    parts.join(" ; ")
}

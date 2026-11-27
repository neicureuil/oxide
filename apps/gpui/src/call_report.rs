//! What the media engine saw during a call, and the Echo Bot test call report (`TestCallReport.swift`).
use crate::oxide::tr;

/// What the media engine saw during a call, for the test call report.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Diagnostics {
    pub mic_permission_denied: bool,
    pub camera_requested: bool,
    pub camera_permission_denied: bool,
    pub camera_failed: bool,
    /// Media reached the connected state at least once.
    pub connected: bool,
    pub disconnects: u32,
    pub failed: bool,
    pub failure_reason: Option<String>,
    /// `totalAudioEnergy` of the microphone source: stays ~0 when nothing is captured.
    pub mic_energy: f64,
    pub audio_packets_received: i64,
    pub audio_packets_lost: i64,
    /// Worst round-trip time reported by the other side, in seconds.
    pub round_trip: f64,
    pub video_frames_sent: i64,
}

impl Diagnostics {
    pub fn merge(&mut self, sample: &Diagnostics) {
        self.mic_energy = self.mic_energy.max(sample.mic_energy);
        self.audio_packets_received = self
            .audio_packets_received
            .max(sample.audio_packets_received);
        self.audio_packets_lost = self.audio_packets_lost.max(sample.audio_packets_lost);
        self.round_trip = self.round_trip.max(sample.round_trip);
        self.video_frames_sent = self.video_frames_sent.max(sample.video_frames_sent);
    }
}

/// What the Mac looked like at the end of the test.
#[derive(Clone, Debug, PartialEq)]
pub struct Context {
    pub microphones: usize,
    pub speakers: usize,
    pub muted_at_end: bool,
    pub output_muted: bool,
    pub has_camera: bool,
}

impl Default for Context {
    fn default() -> Self {
        Self {
            microphones: 1,
            speakers: 1,
            muted_at_end: false,
            output_muted: false,
            has_camera: true,
        }
    }
}

/// Shaped like the web client's `*Errors` lists; empty means no known problem seen, not a guarantee.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Report {
    pub microphone: Vec<String>,
    pub speaker: Vec<String>,
    pub camera: Vec<String>,
    pub network: Vec<String>,
}

impl Report {
    pub fn is_clean(&self) -> bool {
        self.microphone.is_empty()
            && self.speaker.is_empty()
            && self.camera.is_empty()
            && self.network.is_empty()
    }

    pub fn make(d: &Diagnostics, c: &Context) -> Self {
        let mut report = Self::default();
        if d.mic_permission_denied {
            report
                .microphone
                .push(tr("ui.test_call_report.oxide_doesn_t_have_access_to"));
        } else if c.microphones == 0 {
            report
                .microphone
                .push(tr("ui.test_call_report.no_microphone_detected"));
        } else if c.muted_at_end {
            report
                .microphone
                .push(tr("ui.test_call_report.your_mic_was_muted"));
        } else if d.connected && d.mic_energy < 0.0001 {
            report
                .microphone
                .push(tr("ui.test_call_report.the_microphone_didn_t_pick_up"));
        }
        if c.speakers == 0 {
            report
                .speaker
                .push(tr("ui.test_call_report.no_audio_output_detected"));
        } else if c.output_muted {
            report
                .speaker
                .push(tr("ui.test_call_report.the_mac_s_audio_output_is"));
        }
        if d.connected && d.audio_packets_received == 0 {
            report
                .speaker
                .push(tr("ui.test_call_report.no_sound_was_received_from_the"));
        }
        if d.camera_requested {
            if d.camera_permission_denied {
                report
                    .camera
                    .push(tr("ui.test_call_report.oxide_doesn_t_have_access_to_2"));
            } else if !c.has_camera {
                report
                    .camera
                    .push(tr("ui.test_call_report.no_camera_selected"));
            } else if d.camera_failed || (d.connected && d.video_frames_sent == 0) {
                report
                    .camera
                    .push(tr("ui.test_call_report.the_camera_didn_t_start"));
            }
        }
        if !d.connected || d.failed {
            report.network.push(
                d.failure_reason.clone().unwrap_or_else(|| {
                    tr("ui.test_call_report.couldn_t_establish_the_audio_video")
                }),
            );
        } else {
            if d.disconnects > 0 {
                report.network.push(tr(
                    "ui.test_call_report.the_connection_was_interrupted_during_the",
                ));
            }
            let total = d.audio_packets_received + d.audio_packets_lost;
            let loss = if total > 0 {
                d.audio_packets_lost as f64 / total as f64
            } else {
                0.
            };
            if loss > 0.05 || d.round_trip > 0.4 {
                report.network.push(tr(
                    "ui.test_call_report.poor_connection_quality_high_packet_loss",
                ));
            }
        }
        report
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connected() -> Diagnostics {
        Diagnostics {
            connected: true,
            mic_energy: 0.5,
            audio_packets_received: 500,
            ..Default::default()
        }
    }

    #[test]
    fn a_good_call_is_clean() {
        assert!(Report::make(&connected(), &Context::default()).is_clean());
    }

    #[test]
    fn silence_and_no_packets_are_reported() {
        let d = Diagnostics {
            connected: true,
            ..Default::default()
        };
        let report = Report::make(&d, &Context::default());
        assert_eq!(report.microphone.len(), 1);
        assert_eq!(report.speaker.len(), 1);
        assert!(report.network.is_empty());
    }

    #[test]
    fn no_connection_names_the_failure() {
        let d = Diagnostics {
            failure_reason: Some("boom".into()),
            ..Default::default()
        };
        assert_eq!(
            Report::make(&d, &Context::default()).network,
            vec!["boom".to_owned()]
        );
    }

    #[test]
    fn camera_problems_only_when_asked() {
        let mut d = connected();
        assert!(
            Report::make(
                &d,
                &Context {
                    has_camera: false,
                    ..Default::default()
                }
            )
            .camera
            .is_empty()
        );
        d.camera_requested = true;
        assert_eq!(Report::make(&d, &Context::default()).camera.len(), 1);
        d.video_frames_sent = 300;
        assert!(Report::make(&d, &Context::default()).camera.is_empty());
    }

    #[test]
    fn loss_and_latency_flag_the_network() {
        let mut d = connected();
        d.audio_packets_lost = 100;
        assert_eq!(Report::make(&d, &Context::default()).network.len(), 1);
        let mut d = connected();
        d.round_trip = 0.5;
        d.disconnects = 1;
        assert_eq!(Report::make(&d, &Context::default()).network.len(), 2);
    }

    #[test]
    fn merge_keeps_the_maximum() {
        let mut d = connected();
        d.merge(&Diagnostics {
            mic_energy: 2.,
            audio_packets_received: 10,
            round_trip: 0.1,
            ..Default::default()
        });
        assert_eq!(d.mic_energy, 2.);
        assert_eq!(d.audio_packets_received, 500);
        assert_eq!(d.round_trip, 0.1);
    }
}

// Teams fork: the microphone after echo cancellation and noise suppression, for local recordings.
#pragma once

#include <memory>

#include "api/audio/audio_processing.h"

namespace livekit_ffi {

// Capture post-processing stage of the APM: forwards each 10 ms block to Rust while enabled.
std::unique_ptr<webrtc::CustomProcessing> create_mic_tap();

void set_mic_tap_enabled(bool enabled);

}  // namespace livekit_ffi

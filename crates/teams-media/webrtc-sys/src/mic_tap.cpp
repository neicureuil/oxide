// Teams fork: see mic_tap.h.
#include "livekit/mic_tap.h"

#include <algorithm>
#include <atomic>
#include <cmath>
#include <vector>

#include "modules/audio_processing/audio_buffer.h"
#include "webrtc-sys/src/mic_tap.rs.h"

namespace livekit_ffi {

namespace {

std::atomic<bool> g_enabled{false};

class MicTap : public webrtc::CustomProcessing {
 public:
  void Initialize(int sample_rate_hz, int num_channels) override {
    sample_rate_hz_ = sample_rate_hz;
  }

  void Process(webrtc::AudioBuffer* audio) override {
    if (!g_enabled.load(std::memory_order_relaxed))
      return;
    const size_t channels = audio->num_channels();
    const size_t frames = audio->num_frames();
    samples_.resize(channels * frames);
    const float* const* data = audio->channels_const();
    // The APM works on floats in the 16-bit range.
    for (size_t f = 0; f < frames; ++f) {
      for (size_t c = 0; c < channels; ++c) {
        float s = std::clamp(data[c][f], -32768.f, 32767.f);
        samples_[f * channels + c] = static_cast<int16_t>(std::lrintf(s));
      }
    }
    on_mic_pcm(rust::Slice<const int16_t>(samples_.data(), samples_.size()),
               static_cast<uint32_t>(frames * 100),
               static_cast<uint32_t>(channels));
  }

  std::string ToString() const override { return "TeamsMicTap"; }

  void SetRuntimeSetting(
      webrtc::AudioProcessing::RuntimeSetting setting) override {}

 private:
  int sample_rate_hz_ = 48000;
  std::vector<int16_t> samples_;
};

}  // namespace

std::unique_ptr<webrtc::CustomProcessing> create_mic_tap() {
  return std::make_unique<MicTap>();
}

void set_mic_tap_enabled(bool enabled) {
  g_enabled.store(enabled, std::memory_order_relaxed);
}

}  // namespace livekit_ffi

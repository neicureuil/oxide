// Teams fork: the macOS camera through libwebrtc's own AVFoundation capturer (RTCCameraVideoCapturer),
// feeding a VideoTrackSource with the camera's CVPixelBuffers, as the Swift engine did.
#pragma once

#include <atomic>
#include <memory>

#include "livekit/video_track.h"
#include "rust/cxx.h"

namespace livekit_ffi {

class MacCamera {
 public:
  MacCamera(void* capturer, void* delegate, std::shared_ptr<std::atomic<uint64_t>> frames);
  ~MacCamera();
  void stop() const;
  // Frames delivered by AVFoundation so far.
  uint64_t frames() const;

 private:
  void* capturer_;
  void* delegate_;
  std::shared_ptr<std::atomic<uint64_t>> frames_;
};

// Starts the camera whose AVFoundation uniqueID or name matches (else the default one), 720p when
// available; null when there is no camera.
std::unique_ptr<MacCamera> start_mac_camera(std::shared_ptr<VideoTrackSource> source,
                                            rust::String unique_id,
                                            rust::String name);

}  // namespace livekit_ffi

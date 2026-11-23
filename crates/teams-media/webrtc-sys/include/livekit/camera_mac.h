// Teams fork: the macOS camera via libwebrtc's RTCCameraVideoCapturer, CVPixelBuffers into a source.
#pragma once

#include <memory>

#include "livekit/video_track.h"
#include "rust/cxx.h"

namespace livekit_ffi {

class MacCamera {
 public:
  MacCamera(void* capturer, void* delegate);
  ~MacCamera();
  void stop() const;

 private:
  void* capturer_;
  void* delegate_;
};

// The camera matching uniqueID, else name, else the default; null when there is none.
std::unique_ptr<MacCamera> start_mac_camera(std::shared_ptr<VideoTrackSource> source,
                                            rust::String unique_id,
                                            rust::String name);

}  // namespace livekit_ffi

// Teams fork: the macOS camera via libwebrtc's RTCCameraVideoCapturer, CVPixelBuffers into a source.
#pragma once

#include <memory>

#include "livekit/video_track.h"
#include "rust/cxx.h"

namespace livekit_ffi {

struct CameraFailure;

class MacCamera {
 public:
  MacCamera(void* capturer, void* delegate, void* observers);
  ~MacCamera();
  void stop() const;

 private:
  void* capturer_;
  void* delegate_;
  void* observers_;
};

// The camera matching uniqueID, else name, else the default; null when there is none. `failure` runs
// once if the capture can't start or stops on its own (runtime error, unplugged).
std::unique_ptr<MacCamera> start_mac_camera(std::shared_ptr<VideoTrackSource> source,
                                            rust::String unique_id,
                                            rust::String name,
                                            rust::Box<CameraFailure> failure);

}  // namespace livekit_ffi

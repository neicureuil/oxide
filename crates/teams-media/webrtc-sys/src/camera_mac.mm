// Teams fork: see camera_mac.h.
#include "livekit/camera_mac.h"

#import <AVFoundation/AVFoundation.h>
#import <sdk/objc/base/RTCVideoCapturer.h>
#import <sdk/objc/base/RTCVideoFrame.h>
#import <sdk/objc/components/capturer/RTCCameraVideoCapturer.h>

#include "api/video/video_frame.h"
#include "sdk/objc/native/api/video_frame_buffer.h"
#include "webrtc-sys/src/camera_mac.rs.h"

#include <atomic>

@interface TeamsCameraDelegate : NSObject <RTC_OBJC_TYPE(RTCVideoCapturerDelegate)>
- (instancetype)initWithSource:(std::shared_ptr<livekit_ffi::VideoTrackSource>)source;
@end

@implementation TeamsCameraDelegate {
  std::shared_ptr<livekit_ffi::VideoTrackSource> _source;
}

- (instancetype)initWithSource:(std::shared_ptr<livekit_ffi::VideoTrackSource>)source {
  if (self = [super init]) {
    _source = source;
  }
  return self;
}

- (void)capturer:(RTC_OBJC_TYPE(RTCVideoCapturer) *)capturer
    didCaptureVideoFrame:(RTC_OBJC_TYPE(RTCVideoFrame) *)frame {
  webrtc::scoped_refptr<webrtc::VideoFrameBuffer> buffer =
      webrtc::ObjCToNativeVideoFrameBuffer(frame.buffer);
  webrtc::VideoFrame native = webrtc::VideoFrame::Builder()
                                  .set_video_frame_buffer(buffer)
                                  .set_rotation(static_cast<webrtc::VideoRotation>(frame.rotation))
                                  .set_timestamp_us(frame.timeStampNs / 1000)
                                  .build();
  livekit_ffi::FrameMetadata metadata{false, 0, 0, {}};
  _source->get()->on_captured_frame(native, metadata);
}
@end

namespace livekit_ffi {

namespace {

// Reports a capture failure to Rust once, from whichever AVFoundation callback sees it first.
struct FailureOnce {
  explicit FailureOnce(rust::Box<CameraFailure> failure) : failure(std::move(failure)) {}
  void fire() {
    if (!fired.exchange(true)) camera_failed(*failure);
  }
  rust::Box<CameraFailure> failure;
  std::atomic<bool> fired{false};
};

}  // namespace

MacCamera::MacCamera(void* capturer, void* delegate, void* observers)
    : capturer_(capturer), delegate_(delegate), observers_(observers) {}

MacCamera::~MacCamera() {
  for (id observer in (NSArray*)observers_)
    [[NSNotificationCenter defaultCenter] removeObserver:observer];
  [(id)observers_ release];
  stop();
  [(id)capturer_ release];
  [(id)delegate_ release];
}

void MacCamera::stop() const {
  auto capturer = (RTC_OBJC_TYPE(RTCCameraVideoCapturer) *)capturer_;
  dispatch_semaphore_t done = dispatch_semaphore_create(0);
  [capturer stopCaptureWithCompletionHandler:^{
    dispatch_semaphore_signal(done);
  }];
  dispatch_semaphore_wait(done, dispatch_time(DISPATCH_TIME_NOW, 2 * NSEC_PER_SEC));
}

static int32_t width_of(AVCaptureDeviceFormat* format) {
  return CMVideoFormatDescriptionGetDimensions(format.formatDescription).width;
}

std::unique_ptr<MacCamera> start_mac_camera(std::shared_ptr<VideoTrackSource> source,
                                            rust::String unique_id,
                                            rust::String name,
                                            rust::Box<CameraFailure> failure) {
  @autoreleasepool {
    NSArray<AVCaptureDevice*>* devices = [RTC_OBJC_TYPE(RTCCameraVideoCapturer) captureDevices];
    NSString* wanted_id = [NSString stringWithUTF8String:std::string(unique_id).c_str()];
    NSString* wanted_name = [NSString stringWithUTF8String:std::string(name).c_str()];
    AVCaptureDevice* device = nil;
    for (AVCaptureDevice* d in devices)
      if ([d.uniqueID isEqualToString:wanted_id]) device = d;
    if (!device)
      for (AVCaptureDevice* d in devices)
        if ([d.localizedName isEqualToString:wanted_name]) device = d;
    if (!device) device = [AVCaptureDevice defaultDeviceWithMediaType:AVMediaTypeVideo] ?: devices.firstObject;
    if (!device) return nullptr;

    // 720p when available, else the largest format up to 1280 px wide.
    NSArray<AVCaptureDeviceFormat*>* formats =
        [RTC_OBJC_TYPE(RTCCameraVideoCapturer) supportedFormatsForDevice:device];
    AVCaptureDeviceFormat* format = nil;
    for (AVCaptureDeviceFormat* f in formats)
      if (width_of(f) == 1280) { format = f; break; }
    if (!format)
      for (AVCaptureDeviceFormat* f in formats)
        if (width_of(f) <= 1280 && (!format || width_of(f) > width_of(format))) format = f;
    if (!format) format = formats.firstObject;
    if (!format) return nullptr;
    double max_rate = 0;
    for (AVFrameRateRange* range in format.videoSupportedFrameRateRanges)
      max_rate = MAX(max_rate, range.maxFrameRate);
    NSInteger fps = (NSInteger)MIN(30.0, max_rate > 0 ? max_rate : 30.0);

    TeamsCameraDelegate* delegate = [[TeamsCameraDelegate alloc] initWithSource:source];
    RTC_OBJC_TYPE(RTCCameraVideoCapturer)* capturer =
        [[RTC_OBJC_TYPE(RTCCameraVideoCapturer) alloc] initWithDelegate:delegate];
    auto once = std::make_shared<FailureOnce>(std::move(failure));
    [capturer startCaptureWithDevice:device
                              format:format
                                 fps:fps
                   completionHandler:^(NSError* error) {
                     if (error) once->fire();
                   }];
    NSNotificationCenter* center = [NSNotificationCenter defaultCenter];
    NSArray* observers = [@[
      [center addObserverForName:AVCaptureSessionRuntimeErrorNotification
                          object:capturer.captureSession
                           queue:nil
                      usingBlock:^(NSNotification*) { once->fire(); }],
      [center addObserverForName:AVCaptureDeviceWasDisconnectedNotification
                          object:device
                           queue:nil
                      usingBlock:^(NSNotification*) { once->fire(); }],
    ] retain];
    return std::make_unique<MacCamera>((void*)capturer, (void*)delegate, (void*)observers);
  }
}

}  // namespace livekit_ffi

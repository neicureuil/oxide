// Teams fork: see camera_mac.h.
#include "livekit/camera_mac.h"

#import <AVFoundation/AVFoundation.h>
#import <sdk/objc/base/RTCVideoCapturer.h>
#import <sdk/objc/base/RTCVideoFrame.h>
#import <sdk/objc/components/capturer/RTCCameraVideoCapturer.h>

#include "api/video/video_frame.h"
#include "sdk/objc/native/api/video_frame_buffer.h"
#include "webrtc-sys/src/camera_mac.rs.h"

@interface TeamsCameraDelegate : NSObject <RTC_OBJC_TYPE(RTCVideoCapturerDelegate)>
- (instancetype)initWithSource:(std::shared_ptr<livekit_ffi::VideoTrackSource>)source
                        frames:(std::shared_ptr<std::atomic<uint64_t>>)frames;
@end

@implementation TeamsCameraDelegate {
  std::shared_ptr<livekit_ffi::VideoTrackSource> _source;
  std::shared_ptr<std::atomic<uint64_t>> _frames;
}

- (instancetype)initWithSource:(std::shared_ptr<livekit_ffi::VideoTrackSource>)source
                        frames:(std::shared_ptr<std::atomic<uint64_t>>)frames {
  if (self = [super init]) {
    _source = source;
    _frames = frames;
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
  _frames->fetch_add(1, std::memory_order_relaxed);
  livekit_ffi::FrameMetadata metadata{false, 0, 0, {}};
  _source->get()->on_captured_frame(native, metadata);
}
@end

namespace livekit_ffi {

MacCamera::MacCamera(void* capturer,
                     void* delegate,
                     std::shared_ptr<std::atomic<uint64_t>> frames)
    : capturer_(capturer), delegate_(delegate), frames_(frames) {}

uint64_t MacCamera::frames() const {
  return frames_->load(std::memory_order_relaxed);
}

MacCamera::~MacCamera() {
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
                                            rust::String name) {
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

    // 720p when available, else the largest format up to 1280 px wide (the Swift engine's choice).
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

    auto frames = std::make_shared<std::atomic<uint64_t>>(0);
    TeamsCameraDelegate* delegate = [[TeamsCameraDelegate alloc] initWithSource:source frames:frames];
    RTC_OBJC_TYPE(RTCCameraVideoCapturer)* capturer =
        [[RTC_OBJC_TYPE(RTCCameraVideoCapturer) alloc] initWithDelegate:delegate];
    [capturer startCaptureWithDevice:device format:format fps:fps];
    return std::make_unique<MacCamera>((void*)capturer, (void*)delegate, frames);
  }
}

}  // namespace livekit_ffi

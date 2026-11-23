// Video frames of the call media engine outside UniFFI: planes valid only during the callback.
#pragma once

#include <stdint.h>

typedef struct TeamsVideoFrame {
    uint32_t width;
    uint32_t height;
    /// Clockwise degrees: 0, 90, 180, 270.
    uint32_t rotation;
    /// 0: I420 (planes 0-2), 1: NV12 (planes 0-1), 2: pixel_buffer, a CVPixelBufferRef.
    uint32_t format;
    const uint8_t *planes[3];
    uint32_t strides[3];
    void *pixel_buffer;
} TeamsVideoFrame;

typedef void (*TeamsVideoCallback)(void *context, const TeamsVideoFrame *frame);
typedef void (*TeamsVideoRelease)(void *context);

/// Shows a slot's frames through `callback` on a media thread, or stops when it is NULL. `slot`: 0 camera,
/// 1 screen, 2 the remote line `mid`. `release(context)` runs when the sink is replaced or removed.
void teams_media_set_video_sink(uint32_t slot, const char *mid, void *context, TeamsVideoCallback callback,
                                TeamsVideoRelease release);

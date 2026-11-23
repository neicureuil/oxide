import AVFoundation
import os

/// A call video for the window; outputs attach on the main thread, frames arrive on media threads.
protocol VideoFeed: AnyObject {
    func add(_ output: VideoOutput)
    func remove(_ output: VideoOutput)
}

/// Shows frames from any thread, dropping them while the previous one is still on its way to the main thread.
final class VideoOutput: @unchecked Sendable {
    private let layer: AVSampleBufferDisplayLayer
    private let pending = OSAllocatedUnfairLock(initialState: false)
    /// Reused NV12 buffers for frames that aren't one already, rebuilt when the resolution changes.
    private let pool = OSAllocatedUnfairLock<(pool: CVPixelBufferPool, width: Int, height: Int)?>(uncheckedState: nil)
    private let rotation = OSAllocatedUnfairLock(initialState: 0)
    /// The clockwise rotation frames now ask for, told on the main thread when it changes.
    @MainActor var onRotation: ((Int) -> Void)?

    init(layer: AVSampleBufferDisplayLayer) { self.layer = layer }

    /// Shows the frame `make` builds, unless the previous one isn't on screen yet (then `make` isn't called).
    func render(rotation degrees: Int = 0, _ make: () -> CVPixelBuffer?) {
        let changed = rotation.withLock { current in
            defer { current = degrees }
            return current != degrees
        }
        if changed {
            DispatchQueue.main.async { [weak self] in self?.onRotation?(degrees) }
        }
        let busy = pending.withLock { busy in
            defer { busy = true }
            return busy
        }
        guard !busy else { return }
        guard let pixels = make(), let sample = Self.sample(pixels) else {
            pending.withLock { $0 = false }
            return
        }
        DispatchQueue.main.async { [layer, pending] in
            if layer.status == .failed { layer.flush() }
            layer.enqueue(sample)
            pending.withLock { $0 = false }
        }
    }

    /// A pooled NV12 buffer filled with an I420 frame's planes.
    func nv12(width: Int, height: Int,
              y: UnsafePointer<UInt8>, strideY: Int,
              u: UnsafePointer<UInt8>, strideU: Int,
              v: UnsafePointer<UInt8>, strideV: Int) -> CVPixelBuffer? {
        guard let out = makeBuffer(width: width, height: height) else { return nil }
        CVPixelBufferLockBaseAddress(out, [])
        defer { CVPixelBufferUnlockBaseAddress(out, []) }
        guard let dstY = CVPixelBufferGetBaseAddressOfPlane(out, 0)?.assumingMemoryBound(to: UInt8.self),
              let dstUV = CVPixelBufferGetBaseAddressOfPlane(out, 1)?.assumingMemoryBound(to: UInt8.self)
        else { return nil }
        let yStride = CVPixelBufferGetBytesPerRowOfPlane(out, 0)
        let uvStride = CVPixelBufferGetBytesPerRowOfPlane(out, 1)
        for row in 0..<height {
            (dstY + row * yStride).update(from: y + row * strideY, count: width)
        }
        let chromaWidth = (width + 1) / 2, chromaHeight = (height + 1) / 2
        for row in 0..<chromaHeight {
            let rowU = u + row * strideU, rowV = v + row * strideV
            let target = dstUV + row * uvStride
            for column in 0..<chromaWidth {
                target[2 * column] = rowU[column]
                target[2 * column + 1] = rowV[column]
            }
        }
        return out
    }

    /// A pooled NV12 buffer holding a copy of an NV12 frame.
    func nv12(width: Int, height: Int, y: UnsafePointer<UInt8>, strideY: Int,
              uv: UnsafePointer<UInt8>, strideUV: Int) -> CVPixelBuffer? {
        guard let out = makeBuffer(width: width, height: height) else { return nil }
        CVPixelBufferLockBaseAddress(out, [])
        defer { CVPixelBufferUnlockBaseAddress(out, []) }
        guard let dstY = CVPixelBufferGetBaseAddressOfPlane(out, 0)?.assumingMemoryBound(to: UInt8.self),
              let dstUV = CVPixelBufferGetBaseAddressOfPlane(out, 1)?.assumingMemoryBound(to: UInt8.self)
        else { return nil }
        let yStride = CVPixelBufferGetBytesPerRowOfPlane(out, 0)
        let uvStride = CVPixelBufferGetBytesPerRowOfPlane(out, 1)
        for row in 0..<height {
            (dstY + row * yStride).update(from: y + row * strideY, count: width)
        }
        let chromaBytes = (width + 1) / 2 * 2
        for row in 0..<(height + 1) / 2 {
            (dstUV + row * uvStride).update(from: uv + row * strideUV, count: chromaBytes)
        }
        return out
    }

    private func makeBuffer(width: Int, height: Int) -> CVPixelBuffer? {
        pool.withLockUnchecked { current -> CVPixelBuffer? in
            if current?.width != width || current?.height != height {
                let attributes = [
                    kCVPixelBufferPixelFormatTypeKey: kCVPixelFormatType_420YpCbCr8BiPlanarVideoRange,
                    kCVPixelBufferWidthKey: width,
                    kCVPixelBufferHeightKey: height,
                    kCVPixelBufferIOSurfacePropertiesKey: [:] as CFDictionary,
                ] as CFDictionary
                var created: CVPixelBufferPool?
                guard CVPixelBufferPoolCreate(nil, nil, attributes, &created) == kCVReturnSuccess, let created else {
                    current = nil
                    return nil
                }
                current = (created, width, height)
            }
            guard let pool = current?.pool else { return nil }
            var out: CVPixelBuffer?
            guard CVPixelBufferPoolCreatePixelBuffer(nil, pool, &out) == kCVReturnSuccess else { return nil }
            return out
        }
    }

    private static func sample(_ pixels: CVPixelBuffer) -> CMSampleBuffer? {
        var format: CMVideoFormatDescription?
        guard CMVideoFormatDescriptionCreateForImageBuffer(allocator: nil, imageBuffer: pixels,
                                                           formatDescriptionOut: &format) == noErr,
              let format else { return nil }
        var timing = CMSampleTimingInfo(duration: .invalid, presentationTimeStamp: .zero, decodeTimeStamp: .invalid)
        var sample: CMSampleBuffer?
        guard CMSampleBufferCreateReadyWithImageBuffer(allocator: nil, imageBuffer: pixels, formatDescription: format,
                                                       sampleTiming: &timing, sampleBufferOut: &sample) == noErr,
              let sample else { return nil }
        if let attachments = CMSampleBufferGetSampleAttachmentsArray(sample, createIfNecessary: true),
           CFArrayGetCount(attachments) > 0 {
            let dictionary = unsafeBitCast(CFArrayGetValueAtIndex(attachments, 0), to: CFMutableDictionary.self)
            CFDictionarySetValue(dictionary, Unmanaged.passUnretained(kCMSampleAttachmentKey_DisplayImmediately).toOpaque(),
                                 Unmanaged.passUnretained(kCFBooleanTrue).toOpaque())
        }
        return sample
    }
}

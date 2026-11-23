import AVFoundation
import AppKit

/// A layer-backed view fed by a call's video feed.
final class VideoLayerView: NSView {
    private let display = AVSampleBufferDisplayLayer()
    private let output: VideoOutput
    private var feed: VideoFeed?

    /// `fit`: the whole picture, letterboxed, for a shared screen whose edges matter; cameras fill.
    init(mirrored: Bool, fit: Bool = false) {
        output = VideoOutput(layer: display)
        super.init(frame: .zero)
        wantsLayer = true
        layer = CALayer()
        layer?.backgroundColor = NSColor.black.cgColor
        display.videoGravity = fit ? .resizeAspect : .resizeAspectFill
        if mirrored { display.setAffineTransform(CGAffineTransform(scaleX: -1, y: 1)) }
        layer?.addSublayer(display)
    }
    required init?(coder: NSCoder) { fatalError() }

    override func layout() {
        super.layout()
        display.frame = bounds
    }

    func attach(_ next: VideoFeed?) {
        guard next !== feed else { return }
        feed?.remove(output)
        feed = next
        next?.add(output)
    }
}

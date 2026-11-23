import AVFoundation
import AppKit

final class VideoLayerView: NSView {
    private let display = AVSampleBufferDisplayLayer()
    private let output: VideoOutput
    private var feed: VideoFeed?
    private let mirrored: Bool
    /// Clockwise degrees the frames ask to be turned (a phone held upright).
    private var rotation = 0

    /// `fit`: the whole picture, letterboxed, for a shared screen whose edges matter; cameras fill.
    init(mirrored: Bool, fit: Bool = false) {
        output = VideoOutput(layer: display)
        self.mirrored = mirrored
        super.init(frame: .zero)
        wantsLayer = true
        layer = CALayer()
        layer?.backgroundColor = NSColor.black.cgColor
        display.videoGravity = fit ? .resizeAspect : .resizeAspectFill
        layer?.addSublayer(display)
        output.onRotation = { [weak self] degrees in
            self?.rotation = degrees
            self?.needsLayout = true
        }
    }
    required init?(coder: NSCoder) { fatalError() }

    override func layout() {
        super.layout()
        let quarter = rotation % 180 != 0
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        display.setAffineTransform(.identity)
        display.bounds = CGRect(origin: .zero, size: quarter ? CGSize(width: bounds.height, height: bounds.width) : bounds.size)
        display.position = CGPoint(x: bounds.midX, y: bounds.midY)
        var transform = CGAffineTransform(rotationAngle: -CGFloat(rotation) * .pi / 180)
        if mirrored { transform = transform.scaledBy(x: -1, y: 1) }
        display.setAffineTransform(transform)
        CATransaction.commit()
    }

    func attach(_ next: VideoFeed?) {
        guard next !== feed else { return }
        feed?.remove(output)
        feed = next
        next?.add(output)
    }
}

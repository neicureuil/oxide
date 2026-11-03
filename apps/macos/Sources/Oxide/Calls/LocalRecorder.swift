import AVFoundation
import LiveKitWebRTC
import os
import QuartzCore

/// Records the call's audio on this Mac only (not announced to the others): microphone and participants mixed
/// into a 48 kHz mono AAC `.m4a`, streamed to disk as it goes.
///
/// Inputs convert on their audio thread and add into a small ring on a common timeline (seconds since the start);
/// a serial queue writes what no input can still reach to the file.
final class LocalRecorder: @unchecked Sendable {
    static let sampleRate = 48_000.0
    /// Two seconds of mix: inputs add ahead, the writer takes what is older than `latency`.
    private static let capacity = 96_000
    private static let latency: Int64 = 14_400
    /// An input further than this from where the clock puts it jumps there (it paused, or its clock drifted).
    private static let tolerance: Int64 = 4_800

    let url: URL
    let started = Date()
    let format = AVAudioFormat(commonFormat: .pcmFormatFloat32, sampleRate: sampleRate, channels: 1, interleaved: false)!
    private let clock: @Sendable () -> TimeInterval
    private let origin: TimeInterval
    private let queue = DispatchQueue(label: "oxide.local-recorder", qos: .utility)
    private var timer: DispatchSourceTimer?
    /// Queue only.
    private var file: AVAudioFile?
    private let chunk: AVAudioPCMBuffer

    /// Under `lock`: the mix ring, the timeline frame of its first unwritten sample, and where each input has got to.
    private let lock = OSAllocatedUnfairLock()
    private let ring = UnsafeMutablePointer<Float>.allocate(capacity: capacity)
    private var base: Int64 = 0
    private var inputs: [ObjectIdentifier: Int64] = [:]
    private var stopped = false

    /// `clock` is in seconds; tests drive it by hand.
    init(url: URL, clock: @escaping @Sendable () -> TimeInterval = { CACurrentMediaTime() }) throws {
        self.url = url
        self.clock = clock
        origin = clock()
        ring.initialize(repeating: 0, count: Self.capacity)
        chunk = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: AVAudioFrameCount(Self.capacity))!
        let settings: [String: Any] = [
            AVFormatIDKey: kAudioFormatMPEG4AAC,
            AVSampleRateKey: Self.sampleRate,
            AVNumberOfChannelsKey: 1,
            AVEncoderBitRateKey: 64_000,
        ]
        file = try AVAudioFile(forWriting: url, settings: settings, commonFormat: .pcmFormatFloat32, interleaved: false)
        let timer = DispatchSource.makeTimerSource(queue: queue)
        timer.schedule(deadline: .now() + .milliseconds(100), repeating: .milliseconds(100))
        timer.setEventHandler { [weak self] in
            guard let self else { return }
            self.write(upTo: self.now() - Self.latency)
        }
        timer.resume()
        self.timer = timer
    }

    deinit {
        timer?.cancel()
        ring.deallocate()
    }

    /// User defaults key of the folder chosen in Settings; empty or missing means `defaultFolder`.
    static let folderKey = "call.recordings.folder"
    static var defaultFolder: URL {
        FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Documents/Oxide Recordings", isDirectory: true)
    }
    static var folder: URL {
        let path = UserDefaults.standard.string(forKey: folderKey) ?? ""
        return path.isEmpty ? defaultFolder : URL(fileURLWithPath: path, isDirectory: true)
    }

    /// `<folder>/<title> <yyyy-MM-dd HH.mm>.m4a`, never an existing file.
    static func defaultURL(title: String, date: Date = Date(), in directory: URL = folder) throws -> URL {
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.dateFormat = "yyyy-MM-dd HH.mm"
        var name = title.components(separatedBy: CharacterSet(charactersIn: "/:\\")).joined(separator: "-")
            .trimmingCharacters(in: .whitespacesAndNewlines)
        if name.isEmpty { name = "Call" }
        let stem = "\(name.prefix(120)) \(formatter.string(from: date))"
        var url = directory.appendingPathComponent("\(stem).m4a")
        var copy = 2
        while FileManager.default.fileExists(atPath: url.path) {
            url = directory.appendingPathComponent("\(stem) \(copy).m4a")
            copy += 1
        }
        return url
    }

    /// Writes the rest and closes the file.
    @discardableResult
    func stop() -> URL {
        queue.sync {
            timer?.cancel()
            timer = nil
            let end = lock.withLock {
                stopped = true
                return max(inputs.values.max() ?? 0, base)
            }
            write(upTo: max(end, now()))
            // AVAudioFile finishes the file when released.
            file = nil
        }
        return url
    }

    func makeInput() -> Input { Input(self) }

    private func now() -> Int64 {
        Int64(((clock() - origin) * Self.sampleRate).rounded())
    }

    /// Adds `count` samples that end now into the mix (audio thread).
    fileprivate func mix(_ samples: UnsafePointer<Float>, count: Int, from input: ObjectIdentifier) {
        let end = now()
        lock.withLockUnchecked {
            guard !stopped else { return }
            var start = end - Int64(count)
            if let cursor = inputs[input], abs(cursor - start) <= Self.tolerance { start = cursor }
            inputs[input] = start + Int64(count)
            // Too late for the writer, or too far ahead of it: drop those samples.
            let first = max(start, base), last = min(start + Int64(count), base + Int64(Self.capacity))
            guard first < last else { return }
            for frame in first..<last {
                ring[Int(frame % Int64(Self.capacity))] += samples[Int(frame - start)]
            }
        }
    }

    /// Takes the mix before `frame` out of the ring and appends it to the file (queue).
    private func write(upTo frame: Int64) {
        guard let file else { return }
        let count: Int = lock.withLock {
            let count = Int(max(0, min(frame - base, Int64(Self.capacity))))
            let out = chunk.floatChannelData![0]
            for i in 0..<count {
                let slot = Int((base + Int64(i)) % Int64(Self.capacity))
                out[i] = min(1, max(-1, ring[slot]))
                ring[slot] = 0
            }
            base += Int64(count)
            return count
        }
        guard count > 0 else { return }
        chunk.frameLength = AVAudioFrameCount(count)
        do { try file.write(from: chunk) } catch { callTrace("recording write failed: \(error)") }
    }

    /// One source of the mix (a microphone, a remote track), fed from a single audio thread.
    final class Input: @unchecked Sendable {
        private let recorder: LocalRecorder
        private var converter: AVAudioConverter?
        private var output: AVAudioPCMBuffer
        /// The microphone's float samples, rebuilt when its frame size or channels change.
        private var capture: AVAudioPCMBuffer?

        fileprivate init(_ recorder: LocalRecorder) {
            self.recorder = recorder
            output = AVAudioPCMBuffer(pcmFormat: recorder.format, frameCapacity: 4_096)!
        }

        /// Any PCM format; converted to the recorder's.
        func append(_ buffer: AVAudioPCMBuffer) {
            guard buffer.frameLength > 0 else { return }
            if converter?.inputFormat != buffer.format {
                converter = AVAudioConverter(from: buffer.format, to: recorder.format)
                converter?.downmix = true
            }
            guard let converter else { return }
            let needed = AVAudioFrameCount((Double(buffer.frameLength) * LocalRecorder.sampleRate / buffer.format.sampleRate).rounded(.up)) + 64
            if output.frameCapacity < needed {
                output = AVAudioPCMBuffer(pcmFormat: recorder.format, frameCapacity: needed)!
            }
            var given = false
            var error: NSError?
            let status = converter.convert(to: output, error: &error) { _, outStatus in
                if given {
                    outStatus.pointee = .noDataNow
                    return nil
                }
                given = true
                outStatus.pointee = .haveData
                return buffer
            }
            guard status != .error, output.frameLength > 0 else { return }
            recorder.mix(output.floatChannelData![0], count: Int(output.frameLength), from: ObjectIdentifier(self))
        }

        /// A 10 ms block from the audio processing module: floats in the 16-bit range, read only.
        func append(_ buffer: LKRTCAudioBuffer) {
            let channels = AVAudioChannelCount(buffer.channels), frames = AVAudioFrameCount(buffer.frames)
            guard channels > 0, frames > 0 else { return }
            if capture?.format.channelCount != channels || capture?.frameCapacity != frames {
                guard let format = AVAudioFormat(commonFormat: .pcmFormatFloat32, sampleRate: Double(frames) * 100,
                                                 channels: channels, interleaved: false) else { return }
                capture = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: frames)
            }
            guard let capture else { return }
            for channel in 0..<Int(channels) {
                let source = buffer.rawBuffer(forChannel: channel), target = capture.floatChannelData![channel]
                for frame in 0..<Int(frames) { target[frame] = source[frame] / 32_768 }
            }
            capture.frameLength = frames
            append(capture)
        }
    }
}

/// The microphone after libwebrtc's processing: a passthrough that copies into the recording, if any, while unmuted.
final class MicrophoneTap: NSObject, LKRTCAudioCustomProcessingDelegate, @unchecked Sendable {
    private let state = OSAllocatedUnfairLock<(input: LocalRecorder.Input?, muted: Bool)>(initialState: (nil, false))

    var input: LocalRecorder.Input? {
        get { state.withLock { $0.input } }
        set { state.withLock { $0.input = newValue } }
    }
    var muted: Bool {
        get { state.withLock { $0.muted } }
        set { state.withLock { $0.muted = newValue } }
    }

    func audioProcessingInitialize(sampleRate sampleRateHz: Int, channels: Int) {}

    func audioProcessingProcess(audioBuffer: LKRTCAudioBuffer) {
        let (input, muted) = state.withLock { ($0.input, $0.muted) }
        guard let input, !muted else { return }
        input.append(audioBuffer)
    }

    func audioProcessingRelease() {}
}

/// A participants' track into the recording.
final class RemoteAudioTap: NSObject, LKRTCAudioRenderer, @unchecked Sendable {
    let input: LocalRecorder.Input

    init(_ input: LocalRecorder.Input) {
        self.input = input
    }

    func render(pcmBuffer: AVAudioPCMBuffer) {
        input.append(pcmBuffer)
    }
}

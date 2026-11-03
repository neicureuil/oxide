import AVFoundation
import os
import XCTest
@testable import Oxide

final class LocalRecorderTests: XCTestCase {
    /// A microphone at 48 kHz and a participant at 16 kHz stereo 16-bit go into one readable mono AAC file.
    func testMixesInputsIntoAnM4A() throws {
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("oxide-recorder-\(UUID().uuidString).m4a")
        if ProcessInfo.processInfo.environment["OXIDE_KEEP_RECORDING"] == nil {
            addTeardownBlock { try? FileManager.default.removeItem(at: url) }
        }
        let time = OSAllocatedUnfairLock(initialState: 0.0)
        let recorder = try LocalRecorder(url: url, clock: { time.withLock { $0 } })
        let microphone = recorder.makeInput(), participant = recorder.makeInput()

        let micFormat = AVAudioFormat(commonFormat: .pcmFormatFloat32, sampleRate: 48_000, channels: 1, interleaved: false)!
        let remoteFormat = AVAudioFormat(commonFormat: .pcmFormatInt16, sampleRate: 16_000, channels: 2, interleaved: true)!
        let mic = AVAudioPCMBuffer(pcmFormat: micFormat, frameCapacity: 480)!
        let remote = AVAudioPCMBuffer(pcmFormat: remoteFormat, frameCapacity: 160)!
        // Two seconds in 10 ms blocks; the participant is silent during the second one.
        for block in 0..<200 {
            time.withLock { $0 = Double(block + 1) / 100 }
            for i in 0..<480 {
                mic.floatChannelData![0][i] = 0.2 * sin(2 * .pi * 440 * Float(block * 480 + i) / 48_000)
            }
            mic.frameLength = 480
            microphone.append(mic)
            guard block < 100 else { continue }
            for i in 0..<160 {
                let sample = Int16(8_000 * sin(2 * .pi * 220 * Float(block * 160 + i) / 16_000))
                remote.int16ChannelData![0][2 * i] = sample
                remote.int16ChannelData![0][2 * i + 1] = sample
            }
            remote.frameLength = 160
            participant.append(remote)
        }
        XCTAssertEqual(recorder.stop(), url)

        let file = try AVAudioFile(forReading: url)
        XCTAssertEqual(file.fileFormat.sampleRate, 48_000)
        XCTAssertEqual(file.fileFormat.channelCount, 1)
        XCTAssertEqual(Double(file.length) / 48_000, 2, accuracy: 0.1)
        let read = AVAudioPCMBuffer(pcmFormat: file.processingFormat, frameCapacity: AVAudioFrameCount(file.length))!
        try file.read(into: read)
        let samples = UnsafeBufferPointer(start: read.floatChannelData![0], count: Int(read.frameLength))
        func peak(_ seconds: ClosedRange<Double>) -> Float {
            samples[Int(seconds.lowerBound * 48_000)..<Int(seconds.upperBound * 48_000)].map(abs).max() ?? 0
        }
        // Both voices, then the microphone alone.
        XCTAssertEqual(peak(0.2...0.8), 0.44, accuracy: 0.08)
        XCTAssertEqual(peak(1.2...1.8), 0.2, accuracy: 0.05)
    }
}

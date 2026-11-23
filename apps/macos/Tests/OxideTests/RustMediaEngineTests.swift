import XCTest
@testable import Oxide

/// Two synthetic Rust engines (no audio device) negotiate through UniFFI and connect.
@MainActor
final class RustMediaEngineTests: XCTestCase {
    func testOfferAnswerConnects() async throws {
        let caller = RustMediaEngine(synthetic: true)
        let callee = RustMediaEngine(synthetic: true)
        var callerEvents: [MediaEvent] = []
        var calleeEvents: [MediaEvent] = []
        caller.onEvent = { callerEvents.append($0) }
        callee.onEvent = { calleeEvents.append($0) }

        caller.prepare(callId: "c", iceServers: [], video: false, muted: false, speaker: true, cameraLines: 1, remoteOffer: nil)
        let offer = try await wait { callerEvents.compactMap(\.localSDP).first }
        XCTAssertTrue(offer.contains("m=audio"))

        callee.prepare(callId: "c", iceServers: [], video: false, muted: false, speaker: true, cameraLines: 1, remoteOffer: offer)
        let answer = try await wait { calleeEvents.compactMap(\.localSDP).first }
        caller.setRemoteDescription(callId: "c", sdp: answer, provisional: false)

        _ = try await wait { callerEvents.contains(where: \.isConnected) ? true : nil }
        _ = try await wait { calleeEvents.contains(where: \.isConnected) ? true : nil }
        XCTAssertTrue(caller.diagnostics.connected)

        caller.close(callId: "c")
        callee.close(callId: "c")
    }

    private struct TimedOut: Error {}

    private func wait<T>(_ probe: () -> T?, timeout: Duration = .seconds(15)) async throws -> T {
        let deadline = ContinuousClock.now + timeout
        while ContinuousClock.now < deadline {
            if let value = probe() { return value }
            try await Task.sleep(for: .milliseconds(50))
        }
        throw TimedOut()
    }
}

private extension MediaEvent {
    var localSDP: String? {
        if case .localDescription(_, let sdp) = self { return sdp }
        return nil
    }
    var isConnected: Bool {
        if case .connected = self { return true }
        return false
    }
}

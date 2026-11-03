import LiveKitWebRTC
import XCTest
@testable import Oxide

@MainActor
final class MediaEngineTests: XCTestCase {
    /// The service refused our camera renegotiation, then sent its own offer: the call must go on.
    func testARemoteOfferWinsOverOurPendingOffer() async throws {
        let engine = WebRTCMediaEngine()
        let config = LKRTCConfiguration()
        config.sdpSemantics = .unifiedPlan
        let connection = try XCTUnwrap(engine.factory.peerConnection(with: config, constraints: engine.constraints, delegate: engine))
        let receiveAudio = LKRTCRtpTransceiverInit()
        receiveAudio.direction = .recvOnly
        connection.addTransceiver(of: .audio, init: receiveAudio)
        engine.callId = "call"
        engine.connection = connection
        var events: [MediaEvent] = []
        engine.onEvent = { events.append($0) }

        engine.renegotiate("call", reason: "camera on")
        try await waitUntil { connection.signalingState == .haveLocalOffer }

        let service = try XCTUnwrap(engine.factory.peerConnection(with: config, constraints: engine.constraints, delegate: nil))
        service.addTransceiver(of: .audio, init: LKRTCRtpTransceiverInit())
        let offer: LKRTCSessionDescription = try await withCheckedThrowingContinuation { continuation in
            service.offer(for: engine.constraints) { description, error in
                if let description { continuation.resume(returning: description) }
                else { continuation.resume(throwing: error ?? CancellationError()) }
            }
        }
        engine.answerRemoteOffer(callId: "call", sdp: offer.sdp)
        try await waitUntil {
            events.contains { if case .renegotiationAnswer = $0 { true } else { false } }
                || events.contains { if case .failed = $0 { true } else { false } }
        }

        XCTAssertFalse(events.contains { if case .failed = $0 { true } else { false } }, "\(events)")
        XCTAssertEqual(connection.signalingState, .stable)
        service.close()
        engine.close(callId: "call")
    }

    /// Out of retries, a rejected camera renegotiation stops the camera instead of showing it on.
    func testARejectedCameraOfferIsRolledBackAndTheCameraStopped() async throws {
        let (engine, connection, service) = try await negotiatedCall()
        var events: [MediaEvent] = []
        engine.onEvent = { events.append($0) }
        let line = try XCTUnwrap(engine.cameraTransceiver)
        let camera = engine.factory.videoTrack(with: engine.factory.videoSource(), trackId: "video0")
        engine.localVideo = camera
        line.sender.track = camera
        line.setDirection(.sendRecv, error: nil)
        engine.renegotiate("call", reason: "camera on")
        try await waitUntil { connection.signalingState == .haveLocalOffer }

        engine.offerRejected(callId: "call", retry: false)
        try await waitUntil { events.contains { if case .cameraStopped = $0 { true } else { false } } }

        XCTAssertEqual(connection.signalingState, .stable)
        XCTAssertEqual(line.direction, .recvOnly)
        XCTAssertNil(line.sender.track)
        XCTAssertNil(engine.localVideo)
        service.close()
        engine.close(callId: "call")
    }

    /// While retries remain, the rejected offer is replaced by a new one.
    func testARejectedOfferIsOfferedAgain() async throws {
        let (engine, connection, service) = try await negotiatedCall()
        var events: [MediaEvent] = []
        engine.onEvent = { events.append($0) }
        engine.renegotiate("call", reason: "camera on")
        try await waitUntil { events.contains { if case .localDescription = $0 { true } else { false } } }
        events.removeAll()

        engine.offerRejected(callId: "call", retry: true)
        try await waitUntil { events.contains { if case .localDescription = $0 { true } else { false } } }

        XCTAssertEqual(connection.signalingState, .haveLocalOffer)
        XCTAssertFalse(events.contains { if case .failed = $0 { true } else { false } }, "\(events)")
        service.close()
        engine.close(callId: "call")
    }

    /// A connection that fails after being up gets new ICE credentials offered instead of ending the call.
    func testAFailureAfterConnectingRestartsIceInsteadOfEndingTheCall() async throws {
        let (engine, connection, service) = try await negotiatedCall()
        var events: [MediaEvent] = []
        engine.onEvent = { events.append($0) }
        engine.diagnostics.connected = true
        let before = try XCTUnwrap(connection.localDescription?.sdp)

        engine.peerConnection(connection, didChange: LKRTCPeerConnectionState.failed)
        try await waitUntil { events.contains { if case .localDescription = $0 { true } else { false } } }

        XCTAssertTrue(engine.reconnecting)
        XCTAssertFalse(events.contains { if case .failed = $0 { true } else { false } }, "\(events)")
        let offer = try XCTUnwrap(connection.localDescription?.sdp)
        XCTAssertNotEqual(Self.iceUfrag(offer), Self.iceUfrag(before))
        service.close()
        engine.close(callId: "call")
        XCTAssertFalse(engine.reconnecting)
    }

    /// Nothing comes back within the time limit: the call ends.
    func testAReconnectionThatRunsOutOfTimeEndsTheCall() async throws {
        let (engine, connection, service) = try await negotiatedCall()
        var events: [MediaEvent] = []
        engine.onEvent = { events.append($0) }
        engine.diagnostics.connected = true
        engine.reconnectTimeLimit = .milliseconds(500)

        engine.peerConnection(connection, didChange: LKRTCPeerConnectionState.failed)
        try await waitUntil { events.contains { if case .failed = $0 { true } else { false } } }

        XCTAssertFalse(engine.reconnecting)
        service.close()
    }

    /// Media that never came up is not retried, as in the web client (`initialReconnectTimeLimit` 0).
    func testAFailureBeforeConnectingEndsTheCall() async throws {
        let (engine, connection, service) = try await negotiatedCall()
        var events: [MediaEvent] = []
        engine.onEvent = { events.append($0) }

        engine.peerConnection(connection, didChange: LKRTCPeerConnectionState.failed)
        try await waitUntil { events.contains { if case .failed = $0 { true } else { false } } }

        XCTAssertFalse(engine.reconnecting)
        service.close()
    }

    private static func iceUfrag(_ sdp: String) -> Substring? {
        sdp.split(separator: "\r\n").first { $0.hasPrefix("a=ice-ufrag:") }
    }

    /// An engine whose first offer (audio and one camera line, receive-only) the service answered.
    private func negotiatedCall() async throws -> (WebRTCMediaEngine, LKRTCPeerConnection, LKRTCPeerConnection) {
        let engine = WebRTCMediaEngine()
        let config = LKRTCConfiguration()
        config.sdpSemantics = .unifiedPlan
        let connection = try XCTUnwrap(engine.factory.peerConnection(with: config, constraints: engine.constraints, delegate: engine))
        let receive = LKRTCRtpTransceiverInit()
        receive.direction = .recvOnly
        connection.addTransceiver(of: .audio, init: receive)
        engine.cameraTransceiver = connection.addTransceiver(of: .video, init: receive)
        engine.callId = "call"
        engine.connection = connection
        let service = try XCTUnwrap(engine.factory.peerConnection(with: config, constraints: engine.constraints, delegate: nil))

        let offer = try await describe { connection.offer(for: engine.constraints, completionHandler: $0) }
        try await apply { connection.setLocalDescription(offer, completionHandler: $0) }
        try await apply { service.setRemoteDescription(offer, completionHandler: $0) }
        let answer = try await describe { service.answer(for: engine.constraints, completionHandler: $0) }
        try await apply { service.setLocalDescription(answer, completionHandler: $0) }
        try await apply { connection.setRemoteDescription(answer, completionHandler: $0) }
        return (engine, connection, service)
    }

    private func describe(_ make: (@escaping (LKRTCSessionDescription?, Error?) -> Void) -> Void) async throws -> LKRTCSessionDescription {
        try await withCheckedThrowingContinuation { continuation in
            make { description, error in
                if let description { continuation.resume(returning: description) }
                else { continuation.resume(throwing: error ?? CancellationError()) }
            }
        }
    }

    private func apply(_ set: (@escaping (Error?) -> Void) -> Void) async throws {
        try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<Void, Error>) in
            set { error in
                if let error { continuation.resume(throwing: error) } else { continuation.resume() }
            }
        }
    }

    private func waitUntil(_ condition: () -> Bool) async throws {
        for _ in 0..<100 where !condition() {
            try await Task.sleep(for: .milliseconds(50))
        }
        XCTAssertTrue(condition(), "timed out")
    }
}

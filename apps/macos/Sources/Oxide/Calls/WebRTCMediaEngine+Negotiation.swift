import LiveKitWebRTC
import OxideCore

extension WebRTCMediaEngine {
    func start(callId: String, iceServers: [IceServerView], video: Bool, cameraLines: Int, remoteOffer: String?) {
        applyAudioDevices()
        let config = LKRTCConfiguration()
        config.iceServers = iceServers.map {
            LKRTCIceServer(urlStrings: $0.urls, username: $0.username, credential: $0.credential)
        }
        config.sdpSemantics = .unifiedPlan
        config.bundlePolicy = .maxBundle
        config.rtcpMuxPolicy = .require
        config.continualGatheringPolicy = .gatherOnce
        guard let connection = factory.peerConnection(with: config, constraints: constraints, delegate: self) else {
            fail(callId, L("ui.media_engine.couldn_t_prepare_the_audio_connection"))
            return
        }
        self.connection = connection
        let audio = factory.audioTrack(with: factory.audioSource(with: constraints), trackId: "audio0")
        add(audio, to: connection, offering: remoteOffer == nil)
        audioTrack = audio
        audio.isEnabled = !startMuted
        if video, let track = makeCameraTrack() {
            add(track, to: connection, offering: remoteOffer == nil)
            localVideo = track
            cameraTransceiver = connection.transceivers.first { $0.sender.track === track }
        }
        if remoteOffer == nil {
            // Like the web client, always offer every camera line and the sharing line: the mixer rejects missing ones.
            for _ in (localVideo == nil ? 0 : 1)..<max(1, cameraLines) {
                let line = addReceiveVideo(to: connection)
                if cameraTransceiver == nil { cameraTransceiver = line }
            }
            sharingTransceiver = addReceiveVideo(to: connection)
        }

        let made: (LKRTCSessionDescription?, Error?) -> Void = { [weak self] description, error in
            Task { @MainActor in
                guard let self, self.callId == callId else { return }
                guard let description, error == nil else {
                    self.fail(callId, L("ui.media_engine.couldn_t_create_the_local_media") + (error?.localizedDescription ?? "?"))
                    return
                }
                connection.setLocalDescription(description) { error in
                    Task { @MainActor in
                        if let error { self.fail(callId, L("ui.media_engine.local_description_rejected") + error.localizedDescription) }
                        else { self.waitForGathering(callId) }
                    }
                }
            }
        }
        if let remoteOffer {
            connection.setRemoteDescription(LKRTCSessionDescription(type: .offer, sdp: remoteOffer)) { [weak self] error in
                Task { @MainActor in
                    guard let self, self.callId == callId else { return }
                    if let error {
                        self.fail(callId, L("ui.media_engine.remote_offer_rejected") + error.localizedDescription)
                        return
                    }
                    connection.answer(for: self.constraints, completionHandler: made)
                }
            }
        } else {
            connection.offer(for: constraints, completionHandler: made)
        }
    }

    private func add(_ track: LKRTCMediaStreamTrack, to connection: LKRTCPeerConnection, offering: Bool) {
        guard offering else {
            connection.add(track, streamIds: ["local"])
            return
        }
        let options = LKRTCRtpTransceiverInit()
        options.direction = .sendRecv
        options.streamIds = ["local"]
        connection.addTransceiver(with: track, init: options)
    }

    @discardableResult
    private func addReceiveVideo(to connection: LKRTCPeerConnection) -> LKRTCRtpTransceiver? {
        let options = LKRTCRtpTransceiverInit()
        options.direction = .recvOnly
        return connection.addTransceiver(of: .video, init: options)
    }

    /// A new local offer for the call as it now is: the core sends it as a renegotiation.
    func renegotiate(_ callId: String, reason: String) {
        guard let connection else { return }
        callTrace("renegotiate: \(reason)")
        connection.offer(for: constraints) { [weak self] description, error in
            Task { @MainActor in
                guard let self, self.callId == callId, let description, error == nil else { return }
                connection.setLocalDescription(description) { error in
                    Task { @MainActor in
                        guard error == nil else { return }
                        self.sentDescription = false
                        self.answeringRenegotiation = false
                        self.waitForGathering(callId)
                    }
                }
            }
        }
    }

    /// NGC takes no trickled candidates: send once gathering completes, or after 3 s with what was gathered.
    private func waitForGathering(_ callId: String) {
        if connection?.iceGatheringState == .complete {
            sendDescription(callId)
            return
        }
        gatheringTimeout?.cancel()
        gatheringTimeout = Task { [weak self] in
            try? await Task.sleep(for: .seconds(3))
            guard !Task.isCancelled else { return }
            self?.sendDescription(callId)
        }
    }

    private func sendDescription(_ callId: String) {
        guard self.callId == callId, !sentDescription, let sdp = connection?.localDescription?.sdp else { return }
        sentDescription = true
        callTrace("local SDP candidates=\(sdp.components(separatedBy: "a=candidate:").count - 1) relay=\(sdp.contains(" typ relay")) gathering=\(String(describing: connection?.iceGatheringState))")
        gatheringTimeout?.cancel()
        // An incoming call (local answer) starts the deadline here; an outgoing one when the remote answer lands.
        if connection?.localDescription?.type == .answer { waitForConnection(callId) }
        if answeringRenegotiation {
            answeringRenegotiation = false
            onEvent?(.renegotiationAnswer(callId: callId, sdp: sdp))
        } else {
            onEvent?(.localDescription(callId: callId, sdp: sdp))
        }
    }

    func setRemoteDescription(callId: String, sdp: String, provisional: Bool) {
        callTrace("remote SDP provisional=\(provisional) candidates=\(sdp.components(separatedBy: "a=candidate:").count - 1)")
        guard self.callId == callId, let connection else { return }
        connection.setRemoteDescription(LKRTCSessionDescription(type: provisional ? .prAnswer : .answer, sdp: sdp)) { [weak self] error in
            Task { @MainActor in
                guard let self, self.callId == callId, self.connection === connection else { return }
                if let error { self.fail(callId, L("ui.media_engine.media_answer_rejected") + error.localizedDescription) }
                else {
                    callTrace("remote SDP accepted")
                    if !provisional { self.waitForConnection(callId) }
                }
            }
        }
    }

    private func waitForConnection(_ callId: String) {
        connectionTimeout?.cancel()
        // While reconnecting, `reconnect` owns the deadline.
        guard connection?.connectionState != .connected, reconnection == nil else { return }
        connectionTimeout = Task { [weak self] in
            try? await Task.sleep(for: .seconds(30))
            guard !Task.isCancelled, let self, self.callId == callId,
                  self.connection?.connectionState != .connected else { return }
            self.diagnostics.failed = true
            self.fail(callId, L("ui.media_engine.media_negotiation_succeeded_but_the_audio"))
        }
    }

    /// Our own offer may still be pending, e.g. a camera renegotiation the service refused (`mediaRejection`):
    /// a connection takes one offer at a time, so ours yields to the service's instead of failing the call.
    func answerRemoteOffer(callId: String, sdp: String) {
        guard self.callId == callId, let connection else { return }
        rollBackLocalOffer(callId, on: connection) {
            self.applyRemoteOffer(callId: callId, sdp: sdp, to: connection)
        }
    }

    /// The service rejected our renegotiation: drop our offer, then offer again, or put the senders back
    /// as last negotiated, like the web client's `rejectNegotiation`.
    func offerRejected(callId: String, retry: Bool) {
        guard self.callId == callId, let connection else { return }
        callTrace("local offer rejected retry=\(retry)")
        rollBackLocalOffer(callId, on: connection) {
            if retry { self.renegotiate(callId, reason: "retry after rejection") }
            else { self.restoreNegotiatedSenders(callId) }
        }
    }

    private func rollBackLocalOffer(_ callId: String, on connection: LKRTCPeerConnection,
                                    then next: @escaping @MainActor () -> Void) {
        guard connection.signalingState == .haveLocalOffer else {
            next()
            return
        }
        callTrace("rolling back the pending local offer")
        connection.setLocalDescription(LKRTCSessionDescription(type: .rollback, sdp: "")) { [weak self] error in
            Task { @MainActor in
                guard let self, self.callId == callId else { return }
                if let error {
                    self.fail(callId, L("ui.media_engine.renegotiation_rejected") + error.localizedDescription)
                    return
                }
                next()
            }
        }
    }

    /// Stops what the rejected offer meant to start sending, and tells the core.
    private func restoreNegotiatedSenders(_ callId: String) {
        if localVideo != nil, !Self.sends(cameraTransceiver) {
            stopCamera()
            if let line = cameraTransceiver, !line.isStopped {
                line.sender.track = nil
                line.setDirection(.recvOnly, error: nil)
            }
            localVideo = nil
            onEvent?(.cameraStopped(callId: callId, reason: L("ui.media_engine.the_service_refused_the_camera")))
        }
        if screenTrack != nil, !Self.sends(sharingTransceiver) {
            stopScreenCapture()
            if let line = sharingTransceiver, !line.isStopped {
                line.sender.track = nil
                line.setDirection(.recvOnly, error: nil)
            }
            onEvent?(.screenShareStopped(callId: callId, reason: L("ui.media_engine.the_service_refused_screen_sharing")))
        }
    }

    /// The last negotiation lets this line send.
    private static func sends(_ line: LKRTCRtpTransceiver?) -> Bool {
        guard let line, !line.isStopped else { return false }
        var current = LKRTCRtpTransceiverDirection.inactive
        guard line.currentDirection(&current) else { return false }
        return current == .sendRecv || current == .sendOnly
    }

    private func applyRemoteOffer(callId: String, sdp: String, to connection: LKRTCPeerConnection) {
        connection.setRemoteDescription(LKRTCSessionDescription(type: .offer, sdp: sdp)) { [weak self] error in
            Task { @MainActor in
                guard let self, self.callId == callId else { return }
                if let error {
                    self.fail(callId, L("ui.media_engine.renegotiation_rejected") + error.localizedDescription)
                    return
                }
                connection.answer(for: self.constraints) { description, error in
                    Task { @MainActor in
                        guard self.callId == callId, let description, error == nil else { return }
                        connection.setLocalDescription(description) { error in
                            Task { @MainActor in
                                guard error == nil else { return }
                                self.sentDescription = false
                                self.answeringRenegotiation = true
                                self.waitForGathering(callId)
                            }
                        }
                    }
                }
            }
        }
    }

    func restartIce(callId: String) {
        guard self.callId == callId, let connection else { return }
        connection.restartIce()
        renegotiate(callId, reason: "ICE restart")
    }

    /// The connection failed after being up, e.g. a Wi-Fi drop: restart ICE, again after 1 s, 2 s… up to 5 s
    /// between attempts, and end the call only if nothing comes back within `reconnectTimeLimit`,
    /// like the web client (`reconnectTimeLimit`, `maxReconnectDelayTime`).
    func reconnect(_ callId: String) {
        guard reconnection == nil else { return }
        callTrace("reconnecting for up to \(reconnectTimeLimit)")
        reconnecting = true
        let deadline = ContinuousClock.now + reconnectTimeLimit
        reconnection = Task { [weak self] in
            var attempt = 0
            while ContinuousClock.now < deadline {
                guard let self, !Task.isCancelled, self.callId == callId, let connection = self.connection else { return }
                var pause = Duration.milliseconds(500)
                if Self.attemptSettled(connection) {
                    attempt += 1
                    callTrace("reconnect attempt \(attempt)")
                    self.restartIce(callId: callId)
                    pause = .seconds(min(attempt, 5))
                }
                try? await Task.sleep(until: min(deadline, .now + pause), clock: .continuous)
            }
            guard let self, !Task.isCancelled, self.callId == callId else { return }
            self.diagnostics.failed = true
            self.fail(callId, L("ui.media_engine.the_audio_video_connection_failed_network"))
        }
    }

    /// No offer in flight and ICE no longer checking: the previous attempt, if any, is over without success.
    private static func attemptSettled(_ connection: LKRTCPeerConnection) -> Bool {
        connection.signalingState == .stable
            && connection.connectionState != .connecting && connection.connectionState != .connected
    }
}

extension WebRTCMediaEngine: LKRTCPeerConnectionDelegate {
    nonisolated func peerConnection(_ peerConnection: LKRTCPeerConnection, didChange stateChanged: LKRTCSignalingState) {}
    nonisolated func peerConnection(_ peerConnection: LKRTCPeerConnection, didAdd stream: LKRTCMediaStream) {}
    nonisolated func peerConnection(_ peerConnection: LKRTCPeerConnection, didRemove stream: LKRTCMediaStream) {}
    nonisolated func peerConnectionShouldNegotiate(_ peerConnection: LKRTCPeerConnection) {}
    nonisolated func peerConnection(_ peerConnection: LKRTCPeerConnection, didChange newState: LKRTCIceConnectionState) {}
    nonisolated func peerConnection(_ peerConnection: LKRTCPeerConnection, didGenerate candidate: LKRTCIceCandidate) {}
    nonisolated func peerConnection(_ peerConnection: LKRTCPeerConnection, didRemove candidates: [LKRTCIceCandidate]) {}
    nonisolated func peerConnection(_ peerConnection: LKRTCPeerConnection, didOpen dataChannel: LKRTCDataChannel) {}

    nonisolated func peerConnection(_ peerConnection: LKRTCPeerConnection, didChange newState: LKRTCIceGatheringState) {
        guard newState == .complete else { return }
        Task { @MainActor in
            guard self.connection === peerConnection, let callId = self.callId else { return }
            self.sendDescription(callId)
        }
    }

    nonisolated func peerConnection(_ peerConnection: LKRTCPeerConnection, didChange newState: LKRTCPeerConnectionState) {
        callTrace("connection state=\(newState.rawValue)")
        Task { @MainActor in
            guard self.connection === peerConnection, let callId = self.callId else { return }
            switch newState {
            case .connected:
                self.connectionTimeout?.cancel()
                if self.reconnecting { callTrace("reconnected") }
                self.reconnection?.cancel()
                self.reconnection = nil
                self.reconnecting = false
                self.diagnostics.connected = true
                self.pollStatistics(callId)
                self.pollSpeakers(callId)
                self.onEvent?(.connected(callId: callId))
            case .disconnected:
                self.diagnostics.disconnects += 1
            case .failed where self.diagnostics.connected:
                self.reconnect(callId)
            case .failed:
                self.diagnostics.failed = true
                self.fail(callId, L("ui.media_engine.the_audio_video_connection_failed_network"))
            default: break
            }
        }
    }

    nonisolated func peerConnection(_ peerConnection: LKRTCPeerConnection, didAdd rtpReceiver: LKRTCRtpReceiver,
                                    streams mediaStreams: [LKRTCMediaStream]) {
        let track = rtpReceiver.track
        let mid = peerConnection.transceivers.first { $0.receiver.receiverId == rtpReceiver.receiverId }?.mid
        Task { @MainActor in
            guard self.connection === peerConnection else { return }
            if let audio = track as? LKRTCAudioTrack { audio.isEnabled = self.speakerOn }
            if let video = track as? LKRTCVideoTrack {
                if self.remoteVideo == nil { self.remoteVideo = video }
                if let mid { self.remoteVideos[mid] = video }
            }
        }
    }
}

import AVFoundation
import LiveKitWebRTC
import Observation
import OxideCore

/// libwebrtc with the devices chosen in Settings.
@MainActor
@Observable
final class WebRTCMediaEngine: NSObject, MediaEngine {
    @ObservationIgnored var onEvent: ((MediaEvent) -> Void)?
    var localVideo: LKRTCVideoTrack?
    var remoteVideo: LKRTCVideoTrack?
    var remoteVideos: [String: LKRTCVideoTrack] = [:]
    var speakingSources: Set<UInt32> = []
    var localSpeaking = false
    var reconnecting = false
    var diagnostics = CallDiagnostics()
    @ObservationIgnored var statsTask: Task<Void, Never>?
    @ObservationIgnored var speakingTask: Task<Void, Never>?
    /// When each CSRC was last heard, so the indicator does not flicker between words.
    @ObservationIgnored var lastHeard: [UInt32: Date] = [:]
    /// Each CSRC's packet timestamp at the previous poll: it moves only while that voice is mixed in.
    @ObservationIgnored var sourceStamps: [UInt32: CFTimeInterval] = [:]

    /// Built for a call and released by `close`: libwebrtc's threads, codecs and audio device stay alive only during calls.
    @ObservationIgnored var factoryStorage: LKRTCPeerConnectionFactory?
    private static let initializeSSL: Void = { _ = LKRTCInitializeSSL() }()
    var factory: LKRTCPeerConnectionFactory {
        if let factoryStorage { return factoryStorage }
        _ = Self.initializeSSL
        let factory = LKRTCPeerConnectionFactory(
            encoderFactory: LKRTCDefaultVideoEncoderFactory(),
            decoderFactory: LKRTCDefaultVideoDecoderFactory()
        )
        factoryStorage = factory
        return factory
    }
    @ObservationIgnored var callId: String?
    @ObservationIgnored var connection: LKRTCPeerConnection?
    @ObservationIgnored var audioTrack: LKRTCAudioTrack?
    @ObservationIgnored var capturer: LKRTCCameraVideoCapturer?
    @ObservationIgnored var screenCapturer: LKRTCDesktopCapturer?
    @ObservationIgnored var chosenScreen: ScreenChoice? {
        didSet { if oldValue?.lists !== chosenScreen?.lists { oldValue?.lists.close() } }
    }
    @ObservationIgnored var screenTrack: LKRTCVideoTrack?
    /// Frame size of our camera and screen tracks, by track id: the scale depends on it.
    @ObservationIgnored var probes: [String: FrameSizeProbe] = [:]
    @ObservationIgnored var frameSizes: [String: CGSize] = [:]
    /// Last bounds the mixer set per line (WebRTC mid), applied again when the size changes.
    @ObservationIgnored var sendLimits: [String: SendLimits] = [:]
    /// The lines our camera and our screen go out on, kept from the offer that created them.
    @ObservationIgnored var cameraTransceiver: LKRTCRtpTransceiver?
    @ObservationIgnored var sharingTransceiver: LKRTCRtpTransceiver?
    @ObservationIgnored var sentDescription = false
    @ObservationIgnored var startMuted = false
    @ObservationIgnored var speakerOn = true
    @ObservationIgnored var answeringRenegotiation = false
    @ObservationIgnored var gatheringTimeout: Task<Void, Never>?
    @ObservationIgnored var connectionTimeout: Task<Void, Never>?
    @ObservationIgnored var reconnection: Task<Void, Never>?
    /// The web client's `reconnectTimeLimit`: how long a call that was connected tries to get its media back.
    @ObservationIgnored var reconnectTimeLimit: Duration = .seconds(90)

    var constraints: LKRTCMediaConstraints {
        LKRTCMediaConstraints(mandatoryConstraints: nil, optionalConstraints: nil)
    }

    func prepare(callId: String, iceServers: [IceServerView], video: Bool, muted: Bool, speaker: Bool,
                 cameraLines: Int, remoteOffer: String?) {
        callTrace("prepare servers=\(iceServers.count) TURN=\(iceServers.contains { $0.credential != nil }) incoming=\(remoteOffer != nil)")
        close(callId: self.callId ?? callId)
        self.callId = callId
        sentDescription = false
        startMuted = muted
        speakerOn = speaker
        diagnostics = CallDiagnostics(cameraRequested: video)
        Task {
            guard await MediaPermissions.request(.audio) else {
                diagnostics.micPermissionDenied = true
                fail(callId, L("ui.media_engine.microphone_access_is_denied_allow_oxide"))
                return
            }
            let camera = video ? await MediaPermissions.request(.video) : false
            if video && !camera { diagnostics.cameraPermissionDenied = true }
            guard self.callId == callId else { return }
            start(callId: callId, iceServers: iceServers, video: camera, cameraLines: cameraLines, remoteOffer: remoteOffer)
        }
    }

    func close(callId: String) {
        guard self.callId == callId else { return }
        gatheringTimeout?.cancel()
        connectionTimeout?.cancel()
        reconnection?.cancel()
        reconnection = nil
        reconnecting = false
        stopCamera()
        stopScreenCapture()
        chosenScreen = nil
        probes = [:]
        frameSizes = [:]
        sendLimits = [:]
        cameraTransceiver = nil
        sharingTransceiver = nil
        connection?.close()
        connection = nil
        audioTrack = nil
        localVideo = nil
        remoteVideo = nil
        remoteVideos = [:]
        statsTask?.cancel()
        statsTask = nil
        speakingTask?.cancel()
        speakingTask = nil
        lastHeard = [:]
        sourceStamps = [:]
        speakingSources = []
        localSpeaking = false
        self.callId = nil
        // One call at a time: nothing else uses the factory now (tracks still shown keep their own reference).
        factoryStorage = nil
    }

    func fail(_ callId: String, _ reason: String) {
        callTrace("failure: \(reason)")
        diagnostics.failureReason = reason
        onEvent?(.failed(callId: callId, reason: reason))
        close(callId: callId)
    }

    /// Samples WebRTC statistics every 2 s while connected (energy, packets, RTT, frames).
    func pollStatistics(_ callId: String) {
        guard statsTask == nil else { return }
        statsTask = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(for: .seconds(2))
                guard let self, self.callId == callId, let connection = self.connection else { return }
                connection.statistics { report in
                    let sample = Self.sample(report)
                    Task { @MainActor in
                        guard self.callId == callId else { return }
                        self.diagnostics.merge(sample)
                    }
                }
            }
        }
    }

    nonisolated private static func sample(_ report: LKRTCStatisticsReport) -> CallDiagnostics {
        var sample = CallDiagnostics()
        func number(_ stat: LKRTCStatistics, _ key: String) -> Double {
            (stat.values[key] as? NSNumber)?.doubleValue ?? 0
        }
        for stat in report.statistics.values {
            let kind = stat.values["kind"] as? String
            switch (stat.type, kind) {
            case ("media-source", "audio"):
                sample.micEnergy = max(sample.micEnergy, number(stat, "totalAudioEnergy"))
            case ("inbound-rtp", "audio"):
                sample.audioPacketsReceived += Int(number(stat, "packetsReceived"))
                sample.audioPacketsLost += Int(max(0, number(stat, "packetsLost")))
            case ("remote-inbound-rtp", _):
                sample.roundTrip = max(sample.roundTrip, number(stat, "roundTripTime"))
            case ("outbound-rtp", "video"):
                sample.videoFramesSent += Int(number(stat, "framesEncoded"))
            default:
                break
            }
        }
        return sample
    }
}

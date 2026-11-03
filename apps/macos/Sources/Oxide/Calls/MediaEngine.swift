import Darwin
import LiveKitWebRTC
import Observation
import OxideCore

func callTrace(_ message: String) {
    let directory = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Library/Logs/Oxide")
    try? FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
    let fd = Darwin.open(directory.appendingPathComponent("calling.log").path, O_WRONLY | O_CREAT | O_APPEND, 0o600)
    guard fd >= 0 else { return }
    defer { Darwin.close(fd) }
    let line = "\(Int(Date().timeIntervalSince1970)) media \(message)\n"
    line.utf8CString.withUnsafeBytes { bytes in
        _ = Darwin.write(fd, bytes.baseAddress, bytes.count - 1)
    }
}

/// What the media engine reports back to the core.
enum MediaEvent {
    case localDescription(callId: String, sdp: String)
    /// Answer to a renegotiation offer from the service.
    case renegotiationAnswer(callId: String, sdp: String)
    case connected(callId: String)
    case failed(callId: String, reason: String)
    /// Screen sharing ended without the user asking (permission refused, capture stopped).
    case screenShareStopped(callId: String, reason: String?)
    /// The camera stopped without the user asking (the service rejected it).
    case cameraStopped(callId: String, reason: String?)
}

/// The mixer's bounds for one of our sending lines (`controlVideoStreaming` `fmtParams`).
struct SendLimits {
    let mid: String
    let sharing: Bool
    /// Macroblocks per frame.
    let maxFs: Int
    let maxFps: Double
    /// Bits per second; 0 leaves the encoder's own.
    let maxBitrate: Int
    let keyframe: Bool
}

/// Audio/video of the active call, driven by core effects; it speaks standard SDP, the core converts to NGC.
@MainActor
protocol MediaEngine: AnyObject {
    var onEvent: ((MediaEvent) -> Void)? { get set }
    /// Local camera and remote video, for the call window.
    var localVideo: LKRTCVideoTrack? { get }
    var remoteVideo: LKRTCVideoTrack? { get }
    /// Remote video tracks by m-line (`mid`): the mixer's cameras and shared screen.
    var remoteVideos: [String: LKRTCVideoTrack] { get }
    /// CSRCs heard in the mixed audio just now: who is speaking.
    var speakingSources: Set<UInt32> { get }
    /// Our microphone picks up a voice.
    var localSpeaking: Bool { get }
    /// The media connection dropped during the call and is being restored.
    var reconnecting: Bool { get }
    /// What the engine saw during the current or last call.
    var diagnostics: CallDiagnostics { get }
    func prepare(callId: String, iceServers: [IceServerView], video: Bool, muted: Bool, speaker: Bool,
                 cameraLines: Int, remoteOffer: String?)
    func setRemoteDescription(callId: String, sdp: String, provisional: Bool)
    /// The service renegotiates: answer `sdp` (an offer).
    func answerRemoteOffer(callId: String, sdp: String)
    /// The service rejected our renegotiation: offer again, or go back to what was last negotiated.
    func offerRejected(callId: String, retry: Bool)
    /// New ICE credentials after a network change; the new offer comes as `localDescription`.
    func restartIce(callId: String)
    func setMuted(callId: String, muted: Bool)
    func setCamera(callId: String, on: Bool)
    /// Captures `chosenScreen` (else the main screen) on the sharing line, or stops; renegotiates either way.
    func setScreenShare(callId: String, on: Bool)
    /// What the next screen share captures, picked in the share sheet; the engine closes its lists.
    var chosenScreen: ScreenChoice? { get set }
    /// The mixer's bounds for one of our sending lines (`controlVideoStreaming`), and a keyframe.
    func applySendLimits(callId: String, _ limits: SendLimits)
    /// Plays or silences the other participants on this device.
    func setSpeaker(callId: String, on: Bool)
    /// When the local recording (this Mac only, audio) began; nil when not recording.
    var localRecordingStarted: Date? { get }
    /// Records the call's microphone and participants into `url` (`.m4a`) until stopped or the call closes.
    func startLocalRecording(callId: String, url: URL) throws
    /// Finishes the file; its location, or nil when nothing was recording.
    func stopLocalRecording() -> URL?
    /// The latest finished local recording (also when the call's end stopped it), until dismissed.
    var lastLocalRecording: URL? { get set }
    func close(callId: String)
}

/// Demo mode: pretends to negotiate so the call flow can be exercised without a network.
@MainActor
@Observable
final class DemoMediaEngine: MediaEngine {
    @ObservationIgnored var onEvent: ((MediaEvent) -> Void)?
    let localVideo: LKRTCVideoTrack? = nil
    let remoteVideo: LKRTCVideoTrack? = nil
    let remoteVideos: [String: LKRTCVideoTrack] = [:]
    let speakingSources: Set<UInt32> = []
    let localSpeaking = false
    let reconnecting = false
    private(set) var diagnostics = CallDiagnostics()

    func prepare(callId: String, iceServers: [IceServerView], video: Bool, muted: Bool, speaker: Bool,
                 cameraLines: Int, remoteOffer: String?) {
        diagnostics = CallDiagnostics(cameraRequested: video)
        let sdp = "v=0\r\no=- 0 0 IN IP4 127.0.0.1\r\ns=-\r\nt=0 0\r\nm=audio 9 UDP/TLS/RTP/SAVPF 111\r\na=rtpmap:111 opus/48000/2\r\n"
        Task {
            try? await Task.sleep(for: .milliseconds(300))
            onEvent?(.localDescription(callId: callId, sdp: sdp))
            // Answering an incoming call: no remote answer follows, media is up now.
            if remoteOffer != nil { connect(callId) }
        }
    }
    func setRemoteDescription(callId: String, sdp: String, provisional: Bool) {
        guard !provisional else { return }
        Task {
            try? await Task.sleep(for: .milliseconds(300))
            connect(callId)
        }
    }
    private func connect(_ callId: String) {
        diagnostics.connected = true
        diagnostics.micEnergy = 0.5
        diagnostics.audioPacketsReceived = 500
        diagnostics.videoFramesSent = diagnostics.cameraRequested ? 300 : 0
        onEvent?(.connected(callId: callId))
    }
    func answerRemoteOffer(callId: String, sdp: String) {
        onEvent?(.renegotiationAnswer(callId: callId, sdp: "v=0\r\no=- 0 1 IN IP4 127.0.0.1\r\ns=-\r\nt=0 0\r\n"))
    }
    func offerRejected(callId: String, retry: Bool) {}
    func restartIce(callId: String) {}
    func setMuted(callId: String, muted: Bool) {}
    func setCamera(callId: String, on: Bool) {}
    func setScreenShare(callId: String, on: Bool) {}
    var chosenScreen: ScreenChoice? {
        didSet { oldValue?.lists.close() }
    }
    func applySendLimits(callId: String, _ limits: SendLimits) {}
    func setSpeaker(callId: String, on: Bool) {}
    @ObservationIgnored private var recorder: LocalRecorder?
    private(set) var localRecordingStarted: Date?
    var lastLocalRecording: URL?
    /// Silence, for as long as it records.
    func startLocalRecording(callId: String, url: URL) throws {
        _ = stopLocalRecording()
        let recorder = try LocalRecorder(url: url)
        self.recorder = recorder
        localRecordingStarted = recorder.started
    }
    func stopLocalRecording() -> URL? {
        defer {
            recorder = nil
            localRecordingStarted = nil
        }
        guard let url = recorder?.stop() else { return nil }
        lastLocalRecording = url
        return url
    }
    func close(callId: String) {
        _ = stopLocalRecording()
    }
}

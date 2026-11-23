import AVFoundation
import Observation
import OxideCore
import OxideCoreFFI
import os

/// The call media of the Rust core (`teams-media`, libwebrtc), driven by the core's media effects.
@MainActor
@Observable
final class RustMediaEngine: MediaEngine {
    @ObservationIgnored var onEvent: ((MediaEvent) -> Void)?
    private(set) var localVideo: VideoFeed?
    private(set) var remoteVideo: VideoFeed?
    private(set) var remoteVideos: [String: VideoFeed] = [:]
    private(set) var speakingSources: Set<UInt32> = []
    private(set) var localSpeaking = false
    private(set) var reconnecting = false
    private(set) var diagnostics = CallDiagnostics()
    @ObservationIgnored var chosenScreen: ScreenChoice?
    private(set) var localRecordingStarted: Date?
    var lastLocalRecording: URL?

    @ObservationIgnored private let core: CallMedia
    /// No audio device and no permission to ask: demo and tests.
    @ObservationIgnored private let synthetic: Bool
    @ObservationIgnored private var callId: String?
    @ObservationIgnored private var recorder: LocalRecorder?
    @ObservationIgnored private let pcm = RecordingTap()
    /// Feeds stay the same objects across events, so the views keep their outputs attached.
    @ObservationIgnored private var feeds: [VideoSlotView: RustVideoFeed] = [:]

    init(synthetic: Bool = false) {
        let listener = EventForwarder()
        self.synthetic = synthetic
        core = CallMedia(listener: listener, synthetic: synthetic)
        listener.engine = self
    }

    // MARK: Commands

    func prepare(callId: String, iceServers: [IceServerView], video: Bool, muted: Bool, speaker: Bool,
                 cameraLines: Int, remoteOffer: String?) {
        callTrace("rust prepare servers=\(iceServers.count) incoming=\(remoteOffer != nil)")
        if let previous = self.callId { close(callId: previous) }
        self.callId = callId
        diagnostics = CallDiagnostics(cameraRequested: video)
        Task {
            let microphone = synthetic ? true : await MediaPermissions.request(.audio)
            guard microphone else {
                diagnostics.micPermissionDenied = true
                fail(callId, L("ui.media_engine.microphone_access_is_denied_allow_oxide"))
                return
            }
            let camera = video && !synthetic ? await MediaPermissions.request(.video) : false
            if video && !camera { diagnostics.cameraPermissionDenied = true }
            guard self.callId == callId else { return }
            core.setDevices(microphone: Self.microphone(), speaker: Self.speaker(), camera: Self.camera())
            core.prepare(callId: callId, iceServers: iceServers, video: camera, muted: muted, speaker: speaker,
                         cameraLines: UInt32(max(0, cameraLines)), remoteOffer: remoteOffer)
        }
    }

    func setRemoteDescription(callId: String, sdp: String, provisional: Bool) {
        core.setRemoteDescription(callId: callId, sdp: sdp, provisional: provisional)
    }

    func answerRemoteOffer(callId: String, sdp: String) {
        core.answerRemoteOffer(callId: callId, sdp: sdp)
    }

    func offerRejected(callId: String, retry: Bool) {
        core.offerRejected(callId: callId, retry: retry)
    }

    func restartIce(callId: String) {
        core.restartIce(callId: callId)
    }

    func setMuted(callId: String, muted: Bool) {
        guard self.callId == callId else { return }
        core.setMuted(callId: callId, muted: muted)
    }

    func setCamera(callId: String, on: Bool) {
        guard self.callId == callId else { return }
        Task {
            if on {
                guard await MediaPermissions.request(.video) else {
                    diagnostics.cameraPermissionDenied = true
                    return
                }
                core.setDevices(microphone: Self.microphone(), speaker: Self.speaker(), camera: Self.camera())
            }
            core.setCamera(callId: callId, on: on)
        }
    }

    func setScreenShare(callId: String, on: Bool) {
        guard self.callId == callId else { return }
        if on, !(CGPreflightScreenCaptureAccess() || CGRequestScreenCaptureAccess()) {
            onEvent?(.screenShareStopped(callId: callId, reason: L("ui.media_engine.allow_oxide_in_system_settings_privacy")))
            return
        }
        let source = chosenScreen?.source
        chosenScreen = nil
        core.setScreenShare(callId: callId, on: on, source: on ? source : nil)
    }

    func applySendLimits(callId: String, _ limits: SendLimits) {
        core.applySendLimits(callId: callId, mid: limits.mid, sharing: limits.sharing, maxFs: UInt32(limits.maxFs),
                             maxFps: limits.maxFps, maxBitrate: UInt32(limits.maxBitrate), keyframe: limits.keyframe)
    }

    func setSpeaker(callId: String, on: Bool) {
        core.setSpeaker(callId: callId, on: on)
    }

    func startLocalRecording(callId: String, url: URL) throws {
        guard self.callId == callId else { throw CocoaError(.featureUnsupported) }
        _ = stopLocalRecording()
        let recorder = try LocalRecorder(url: url)
        self.recorder = recorder
        pcm.start(recorder)
        core.setPcmListener(listener: pcm)
        localRecordingStarted = recorder.started
        callTrace("local recording started")
    }

    func stopLocalRecording() -> URL? {
        guard let recorder else { return nil }
        core.setPcmListener(listener: nil)
        pcm.stop()
        self.recorder = nil
        localRecordingStarted = nil
        callTrace("local recording stopped")
        let url = recorder.stop()
        lastLocalRecording = url
        return url
    }

    func close(callId: String) {
        guard self.callId == callId else { return }
        core.close(callId: callId)
        endCall()
    }

    private func endCall() {
        _ = stopLocalRecording()
        chosenScreen = nil
        callId = nil
        reset()
    }

    private func reset() {
        for feed in feeds.values { feed.detach() }
        feeds = [:]
        localVideo = nil
        remoteVideo = nil
        remoteVideos = [:]
        speakingSources = []
        localSpeaking = false
        reconnecting = false
    }

    private func fail(_ callId: String, _ reason: String) {
        diagnostics.failureReason = reason
        onEvent?(.failed(callId: callId, reason: reason))
        close(callId: callId)
    }

    // MARK: Events

    fileprivate func handle(_ event: MediaEventView) {
        switch event {
        case .localDescription(let callId, let sdp):
            guard callId == self.callId else { return }
            onEvent?(.localDescription(callId: callId, sdp: sdp))
        case .renegotiationAnswer(let callId, let sdp):
            guard callId == self.callId else { return }
            onEvent?(.renegotiationAnswer(callId: callId, sdp: sdp))
        case .connected(let callId):
            guard callId == self.callId else { return }
            diagnostics.connected = true
            onEvent?(.connected(callId: callId))
        case .failed(let callId, let reason):
            guard callId == self.callId else { return }
            diagnostics.failureReason = reason
            onEvent?(.failed(callId: callId, reason: reason))
            endCall()
        case .screenShareStopped(let callId, let reason):
            onEvent?(.screenShareStopped(callId: callId, reason: reason))
        case .cameraStopped(let callId, let reason):
            onEvent?(.cameraStopped(callId: callId, reason: reason))
        case .speaking(let callId, let sources):
            guard callId == self.callId else { return }
            speakingSources = Set(sources)
        case .localSpeaking(let callId, let speaking):
            guard callId == self.callId else { return }
            localSpeaking = speaking
        case .reconnecting(let callId, let reconnecting):
            guard callId == self.callId else { return }
            self.reconnecting = reconnecting
        case .video(let callId, let slot, let present):
            guard callId == self.callId else { return }
            video(slot, present: present)
        case .diagnostics(let callId, let sample):
            guard callId == self.callId else { return }
            diagnostics.merge(sample)
        }
    }

    private func video(_ slot: VideoSlotView, present: Bool) {
        if present {
            let feed = feeds[slot] ?? RustVideoFeed(slot)
            feeds[slot] = feed
            switch slot {
            case .camera: localVideo = feed
            case .screen: break
            case .remote(let mid):
                remoteVideos[mid] = feed
                if remoteVideo == nil { remoteVideo = feed }
            }
        } else {
            feeds.removeValue(forKey: slot)?.detach()
            switch slot {
            case .camera: localVideo = nil
            case .screen: break
            case .remote(let mid):
                let gone = remoteVideos.removeValue(forKey: mid)
                if remoteVideo === gone { remoteVideo = remoteVideos.values.first }
            }
        }
    }

    // MARK: Devices

    /// The microphone chosen in Settings, by CoreAudio id (the audio device module's) and name; nil for the default.
    private static func microphone() -> DeviceChoiceView? {
        guard let id = MediaDevicePreferences.resolvedMicrophone(), id != CoreAudioDevices.defaultDevice(input: true) else { return nil }
        return DeviceChoiceView(id: String(id), name: CoreAudioDevices.name(id) ?? "")
    }

    private static func speaker() -> DeviceChoiceView? {
        guard MediaDevicePreferences.resolvedSpeakerUID() != nil, let id = MediaDevicePreferences.resolvedSpeaker() else { return nil }
        return DeviceChoiceView(id: String(id), name: CoreAudioDevices.name(id) ?? "")
    }

    private static func camera() -> DeviceChoiceView? {
        guard let device = MediaDevicePreferences.resolvedCamera() else { return nil }
        return DeviceChoiceView(id: device.uniqueID, name: device.localizedName)
    }
}

private extension CallDiagnostics {
    mutating func merge(_ sample: CallDiagnosticsView) {
        if sample.cameraFailed { cameraFailed = true }
        disconnects = max(disconnects, Int(sample.disconnects))
        merge(CallDiagnostics(micEnergy: sample.micEnergy, audioPacketsReceived: Int(sample.audioPacketsReceived),
                              audioPacketsLost: Int(sample.audioPacketsLost), roundTrip: sample.roundTrip,
                              videoFramesSent: Int(sample.videoFramesSent)))
    }
}

/// Engine events arrive on Rust threads; the engine lives on the main actor.
private final class EventForwarder: MediaListener, @unchecked Sendable {
    @MainActor weak var engine: RustMediaEngine?

    func onEvent(event: MediaEventView) {
        Task { @MainActor [weak self] in self?.engine?.handle(event) }
    }
}

/// The microphone (the engine stops it while muted) and the participants into the local recording.
private final class RecordingTap: PcmListener, @unchecked Sendable {
    private let state = OSAllocatedUnfairLock<(mic: LocalRecorder.Input?, remote: LocalRecorder.Input?)>(
        initialState: (nil, nil))

    func start(_ recorder: LocalRecorder) {
        state.withLock { $0.mic = recorder.makeInput(); $0.remote = recorder.makeInput() }
    }

    func stop() {
        state.withLock { $0.mic = nil; $0.remote = nil }
    }

    func onPcm(remote: Bool, samples: [Int16], sampleRate: UInt32, channels: UInt32) {
        let input = state.withLock { remote ? $0.remote : $0.mic }
        guard let input, channels > 0, !samples.isEmpty,
              let format = AVAudioFormat(commonFormat: .pcmFormatInt16, sampleRate: Double(sampleRate),
                                         channels: AVAudioChannelCount(channels), interleaved: true)
        else { return }
        let frames = AVAudioFrameCount(samples.count / Int(channels))
        guard let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: frames) else { return }
        buffer.frameLength = frames
        samples.withUnsafeBytes { bytes in
            buffer.mutableAudioBufferList.pointee.mBuffers.mData?.copyMemory(from: bytes.baseAddress!, byteCount: bytes.count)
        }
        input.append(buffer)
    }
}

/// One slot of the Rust engine, fanned out to the views showing it. Frames come from the C callback.
final class RustVideoFeed: VideoFeed, @unchecked Sendable {
    private let slot: VideoSlotView
    private let outputs = OSAllocatedUnfairLock<[ObjectIdentifier: VideoOutput]>(initialState: [:])
    private var registered = false

    init(_ slot: VideoSlotView) { self.slot = slot }

    func add(_ output: VideoOutput) {
        outputs.withLock { $0[ObjectIdentifier(output)] = output }
        guard !registered else { return }
        registered = true
        let context = Unmanaged.passRetained(self).toOpaque()
        sink(context: context, callback: { context, frame in
            guard let context, let frame else { return }
            Unmanaged<RustVideoFeed>.fromOpaque(context).takeUnretainedValue().deliver(frame.pointee)
        }, release: { context in
            guard let context else { return }
            Unmanaged<RustVideoFeed>.fromOpaque(context).release()
        })
    }

    func remove(_ output: VideoOutput) {
        let empty = outputs.withLock { outputs in
            outputs.removeValue(forKey: ObjectIdentifier(output))
            return outputs.isEmpty
        }
        if empty { detach() }
    }

    /// Stops the frames (the slot went away, or nobody shows it).
    func detach() {
        guard registered else { return }
        registered = false
        sink(context: nil, callback: nil, release: nil)
    }

    private func sink(context: UnsafeMutableRawPointer?, callback: TeamsVideoCallback?, release: TeamsVideoRelease?) {
        switch slot {
        case .camera: teams_media_set_video_sink(0, nil, context, callback, release)
        case .screen: teams_media_set_video_sink(1, nil, context, callback, release)
        case .remote(let mid): mid.withCString { teams_media_set_video_sink(2, $0, context, callback, release) }
        }
    }

    private func deliver(_ frame: TeamsVideoFrame) {
        let outputs = outputs.withLock { Array($0.values) }
        for output in outputs {
            output.render(rotation: Int(frame.rotation)) {
                let width = Int(frame.width), height = Int(frame.height)
                switch frame.format {
                case 2:
                    guard let pixels = frame.pixel_buffer else { return nil }
                    return Unmanaged<CVPixelBuffer>.fromOpaque(pixels).takeUnretainedValue()
                case 1:
                    guard let y = frame.planes.0, let uv = frame.planes.1 else { return nil }
                    return output.nv12(width: width, height: height, y: y, strideY: Int(frame.strides.0),
                                       uv: uv, strideUV: Int(frame.strides.1))
                default:
                    guard let y = frame.planes.0, let u = frame.planes.1, let v = frame.planes.2 else { return nil }
                    return output.nv12(width: width, height: height, y: y, strideY: Int(frame.strides.0),
                                       u: u, strideU: Int(frame.strides.1), v: v, strideV: Int(frame.strides.2))
                }
            }
        }
    }
}

import CoreAudio
import Foundation
import LiveKitWebRTC

extension WebRTCMediaEngine {
    func setMuted(callId: String, muted: Bool) {
        guard self.callId == callId else { return }
        audioTrack?.isEnabled = !muted
        microphoneTap.muted = muted
    }

    func startLocalRecording(callId: String, url: URL) throws {
        guard self.callId == callId, let connection else { throw CocoaError(.featureUnsupported) }
        _ = stopLocalRecording()
        let recorder = try LocalRecorder(url: url)
        self.recorder = recorder
        microphoneTap.muted = audioTrack?.isEnabled != true
        microphoneTap.input = recorder.makeInput()
        connection.receivers.compactMap { $0.track as? LKRTCAudioTrack }.forEach(recordRemote)
        localRecordingStarted = recorder.started
        callTrace("local recording started")
    }

    func stopLocalRecording() -> URL? {
        guard let recorder else { return nil }
        microphoneTap.input = nil
        remoteTaps.forEach { $0.track.remove($0.tap) }
        remoteTaps = []
        self.recorder = nil
        localRecordingStarted = nil
        callTrace("local recording stopped")
        let url = recorder.stop()
        lastLocalRecording = url
        return url
    }

    /// Adds a participants' track to the recording in progress.
    func recordRemote(_ track: LKRTCAudioTrack) {
        guard let recorder, !remoteTaps.contains(where: { $0.track === track }) else { return }
        let tap = RemoteAudioTap(recorder.makeInput())
        track.add(tap)
        remoteTaps.append((track, tap))
    }

    func setSpeaker(callId: String, on: Bool) {
        guard self.callId == callId else { return }
        speakerOn = on
        connection?.receivers.compactMap { $0.track as? LKRTCAudioTrack }.forEach { $0.isEnabled = on }
    }

    /// Every 250 ms: the CSRCs the mixer currently mixes in, and our own input level.
    func pollSpeakers(_ callId: String) {
        guard speakingTask == nil else { return }
        speakingTask = Task { [weak self] in
            while !Task.isCancelled {
                try? await Task.sleep(for: .milliseconds(250))
                guard let self, self.callId == callId, let connection = self.connection else { return }
                let sources = connection.receivers.filter { $0.track is LKRTCAudioTrack }.flatMap(\.sources)
                // libwebrtc keeps a source 10 s after its last packet: only a moving timestamp means it is mixed in now.
                let now = Date()
                for source in sources where source.sourceType == .CSRC {
                    let previous = self.sourceStamps.updateValue(source.timestampUs, forKey: source.sourceId)
                    if let previous, previous != source.timestampUs,
                       (source.audioLevel?.doubleValue ?? 1) > 0.01 {
                        self.lastHeard[source.sourceId] = now
                    }
                }
                self.lastHeard = self.lastHeard.filter { now.timeIntervalSince($0.value) < 0.8 }
                let speaking = Set(self.lastHeard.keys)
                if speaking != self.speakingSources {
                    callTrace("speaking CSRCs=\(speaking.sorted()) of \(sources.filter { $0.sourceType == .CSRC }.count)")
                    self.speakingSources = speaking
                }
                connection.statistics { report in
                    let level = report.statistics.values
                        .first { $0.type == "media-source" && $0.values["kind"] as? String == "audio" }
                        .flatMap { ($0.values["audioLevel"] as? NSNumber)?.doubleValue } ?? 0
                    Task { @MainActor in
                        guard self.callId == callId else { return }
                        let speaking = level > 0.05 && self.audioTrack?.isEnabled == true
                        if speaking != self.localSpeaking { self.localSpeaking = speaking }
                    }
                }
            }
        }
    }

    /// Microphone and output chosen in Settings, matched by CoreAudio UID then by name.
    func applyAudioDevices() {
        let module = factory.audioDeviceModule
        if let device = Self.device(MediaDevicePreferences.resolvedMicrophone(), in: module.inputDevices) {
            _ = module.trySetInputDevice(device)
        }
        if let device = Self.device(MediaDevicePreferences.resolvedSpeaker(), in: module.outputDevices) {
            _ = module.trySetOutputDevice(device)
        }
    }

    private static func device(_ id: AudioDeviceID?, in devices: [LKRTCIODevice]) -> LKRTCIODevice? {
        guard let id, let uid = CoreAudioDevices.uid(id) else { return nil }
        let name = CoreAudioDevices.name(id)
        return devices.first { $0.deviceId == uid || $0.name == name }
    }
}

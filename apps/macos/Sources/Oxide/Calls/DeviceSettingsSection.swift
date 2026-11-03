import AVFoundation
import SwiftUI

/// Settings › Audio et vidéo: default call devices, with a test for each, and the local recordings folder.
/// One card per device, laid out like System Settings: title on the left, control on the right.
struct DeviceSettingsSection: View {
    @AppStorage(MediaDevicePreferences.microphoneKey) private var microphone = ""
    @AppStorage(MediaDevicePreferences.speakerKey) private var speaker = ""
    @AppStorage(MediaDevicePreferences.cameraKey) private var camera = ""
    @AppStorage(LocalRecorder.folderKey) private var recordingsFolder = ""
    @State private var devices = MediaDevices()
    @State private var mic = MicrophoneTester()
    @State private var sound = SpeakerTester()
    @State private var previewing = false
    @State private var cameraDenied = false

    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            microphoneCard
            speakerCard
            cameraCard
            TestCallRow(onStart: {
                mic.stop()
                previewing = false
            })
            .card()
            recordingsCard
            Muted(L("ui.device_settings_section.default_devices_detail"), size: 12)
                .padding(.horizontal, 4)
        }
        .onDisappear {
            mic.stop()
            previewing = false
        }
    }

    // MARK: Microphone

    private var microphoneCard: some View {
        VStack(spacing: 0) {
            DeviceRow(L("ui.device_settings_section.microphone"), symbol: "mic") {
                picker(L("ui.device_settings_section.microphone"), selection: $microphone,
                       options: devices.microphones.map { ($0.uid, $0.name) },
                       systemName: devices.defaultMicrophone)
                    .onChange(of: microphone) { _, _ in restartMicrophone() }
            }
            Divider().padding(.leading, DeviceRow<EmptyView>.inset)
            DeviceRow(L("ui.device_settings_section.input_level")) {
                HStack(spacing: 8) {
                    LevelMeter(tester: mic)
                        .frame(width: 140, height: 6)
                        .padding(.trailing, 4)
                    Button(mic.running ? L("ui.device_settings_section.stop") : L("ui.device_settings_section.test_mic")) {
                        if mic.running { mic.stop() } else { Task { await mic.start() } }
                    }
                    .buttonStyle(OutlineButtonStyle())
                    Button(mic.recording ? L("ui.device_settings_section.recording") : mic.playingBack ? L("ui.device_settings_section.playing") : L("ui.device_settings_section.hear_yourself_5_s")) {
                        Task { await mic.echo() }
                    }
                    .buttonStyle(OutlineButtonStyle())
                    .disabled(mic.recording || mic.playingBack)
                    .help(L("ui.device_settings_section.records_5_seconds_then_plays_them"))
                }
            }
            if let error = mic.error { problem(error, settings: .audio) }
        }
        .card()
    }

    private func restartMicrophone() {
        guard mic.running else { return }
        mic.stop()
        Task { await mic.start() }
    }

    // MARK: Speaker

    private var speakerCard: some View {
        VStack(spacing: 0) {
            DeviceRow(L("ui.device_settings_section.speaker"), symbol: "speaker.wave.2") {
                picker(L("ui.device_settings_section.speaker"), selection: $speaker,
                       options: devices.speakers.map { ($0.uid, $0.name) },
                       systemName: devices.defaultSpeaker)
            }
            Divider().padding(.leading, DeviceRow<EmptyView>.inset)
            DeviceRow(L("ui.device_settings_section.test_sound"), detail: sound.error) {
                Button(sound.playing ? L("ui.device_settings_section.playing") : L("ui.device_settings_section.test_speaker")) { sound.play() }
                    .buttonStyle(OutlineButtonStyle())
                    .disabled(sound.playing)
            }
        }
        .card()
    }

    // MARK: Local recordings

    private var recordingsCard: some View {
        DeviceRow(L("ui.device_settings_section.local_recordings"), symbol: "record.circle",
                  detail: (recordingsFolder.isEmpty ? LocalRecorder.defaultFolder.path : recordingsFolder)
                      .replacingOccurrences(of: NSHomeDirectory(), with: "~"),
                  detailIsError: false) {
            if !recordingsFolder.isEmpty {
                Button(L("ui.device_settings_section.reset_folder")) { recordingsFolder = "" }
                    .buttonStyle(OutlineButtonStyle())
            }
            Button(L("ui.device_settings_section.change_folder")) { chooseRecordingsFolder() }
                .buttonStyle(OutlineButtonStyle())
        }
        .card()
    }

    private func chooseRecordingsFolder() {
        let panel = NSOpenPanel()
        panel.canChooseDirectories = true
        panel.canChooseFiles = false
        panel.canCreateDirectories = true
        panel.prompt = L("ui.device_settings_section.choose_folder")
        panel.directoryURL = LocalRecorder.folder
        if panel.runModal() == .OK, let url = panel.url { recordingsFolder = url.path }
    }

    // MARK: Camera

    private var cameraCard: some View {
        VStack(spacing: 0) {
            DeviceRow(L("ui.device_settings_section.camera"), symbol: "video") {
                picker(L("ui.device_settings_section.camera"), selection: $camera,
                       options: devices.cameras.map { ($0.uniqueID, $0.name) },
                       systemName: AVCaptureDevice.default(for: .video)?.localizedName)
            }
            ZStack {
                RoundedRectangle(cornerRadius: 10).fill(Theme.input)
                if previewing {
                    CameraPreview(device: selectedCamera)
                        .clipShape(RoundedRectangle(cornerRadius: 10))
                } else {
                    Image(systemName: "video.slash").font(.system(size: 22)).foregroundStyle(Theme.subtle)
                }
            }
            .frame(height: 220)
            .overlay(alignment: .bottomTrailing) {
                Button(previewing ? L("ui.device_settings_section.stop_preview") : L("ui.device_settings_section.preview")) {
                    if previewing { previewing = false } else {
                        Task {
                            cameraDenied = !(await MediaPermissions.request(.video))
                            previewing = !cameraDenied
                        }
                    }
                }
                .buttonStyle(OutlineButtonStyle())
                .padding(10)
            }
            .padding([.horizontal, .bottom], 16)
            if cameraDenied {
                problem(L("ui.device_settings_section.camera_access_is_denied_allow_oxide"),
                        settings: .video)
            }
        }
        .card()
    }

    private var selectedCamera: AVCaptureDevice? { devices.camera(preferred: camera) }

    // MARK: Shared

    private func picker(_ title: String, selection: Binding<String>,
                        options: [(id: String, name: String)], systemName: String?) -> some View {
        Picker(title, selection: selection) {
            Text(systemName.map { L("ui.device_settings_section.system_default", ["device": $0]) } ?? L("ui.device_settings_section.system_default_2")).tag("")
            ForEach(options, id: \.id) { option in Text(option.name).tag(option.id) }
            if !selection.wrappedValue.isEmpty && !options.contains(where: { $0.id == selection.wrappedValue }) {
                Text(L("ui.device_settings_section.device_not_connected")).tag(selection.wrappedValue)
            }
        }
        .labelsHidden()
        .fixedSize()
    }

    private func problem(_ text: String, settings type: AVMediaType) -> some View {
        HStack(spacing: 8) {
            Text(text).font(.figtree(12)).foregroundStyle(Theme.danger)
            Button(L("ui.device_settings_section.open_settings")) { NSWorkspace.shared.open(MediaPermissions.settingsURL(type)) }
                .buttonStyle(.link)
            Spacer(minLength: 0)
        }
        .padding(.leading, DeviceRow<EmptyView>.inset)
        .padding([.trailing, .bottom], 16)
    }
}

/// One line of a device card: icon and title (and an optional detail line) on the left,
/// the control pushed to the right edge. Lines without an icon keep its slot, so titles align.
private struct DeviceRow<Trailing: View>: View {
    /// Where titles start: padding, icon slot, spacing.
    static var inset: CGFloat { 16 + 18 + 10 }

    let title: String
    var symbol: String?
    var detail: String?
    var detailIsError = true
    @ViewBuilder let trailing: Trailing

    init(_ title: String, symbol: String? = nil, detail: String? = nil, detailIsError: Bool = true,
         @ViewBuilder trailing: () -> Trailing) {
        self.title = title
        self.symbol = symbol
        self.detail = detail
        self.detailIsError = detailIsError
        self.trailing = trailing()
    }

    var body: some View {
        HStack(spacing: 10) {
            Group {
                if let symbol { Image(systemName: symbol).foregroundStyle(Theme.muted) } else { Color.clear }
            }
            .frame(width: 18)
            VStack(alignment: .leading, spacing: 3) {
                Text(title).font(.figtree(13, symbol == nil ? .regular : .semibold))
                if let detail {
                    Text(detail)
                        .font(.figtree(12))
                        .foregroundStyle(detailIsError ? Theme.danger : Theme.muted)
                        .fixedSize(horizontal: false, vertical: true)
                }
            }
            Spacer(minLength: 16)
            trailing
        }
        .padding(.horizontal, 16)
        .frame(minHeight: 52)
    }
}

/// Reads the level itself, so only the meter is redrawn at its rate, not the whole section.
private struct LevelMeter: View {
    let tester: MicrophoneTester
    var body: some View {
        let level = tester.level, active = tester.running
        GeometryReader { geometry in
            ZStack(alignment: .leading) {
                Capsule().fill(Theme.input)
                Capsule()
                    .fill(level > 0.85 ? Theme.danger : Theme.accent)
                    .frame(width: geometry.size.width * CGFloat(active ? level : 0))
                    .animation(.linear(duration: 0.08), value: level)
            }
        }
        .accessibilityLabel(L("ui.device_settings_section.mic_level"))
        .accessibilityValue(L("ui.device_settings_section.text", ["percent": Int(level * 100)]))
    }
}

enum TestCall {
    /// Optional override of the Echo Bot MRI (`defaults write fr.neicureuil.oxide call.echoBotMri 28:…`).
    static var botOverride: String? {
        UserDefaults.standard.string(forKey: "call.echoBotMri").flatMap { $0.isEmpty ? nil : $0 }
    }
}

/// Settings › Audio et vidéo › Appel de test: a real call with the Echo Bot, then a report.
private struct TestCallRow: View {
    @Environment(Store.self) private var store
    let onStart: () -> Void

    /// The core's status text once registered for calls.
    private var ready: Bool {
        store.calls?.realtime == L("core.calls.ready_to_receive_calls")
    }
    private var busy: Bool { store.inCall }

    var body: some View {
        DeviceRow(L("ui.device_settings_section.test_call"), symbol: "phone.badge.waveform",
                  detail: hint, detailIsError: false) {
            Button(L("ui.device_settings_section.make_a_test_call")) {
                onStart()
                store.send(.call(action: .testCall(botMri: TestCall.botOverride)))
            }
            .buttonStyle(OutlineButtonStyle())
            .disabled(!ready || busy)
        }
        .padding(.vertical, 12)
    }

    private var hint: String {
        if !ready {
            let status = store.calls?.realtime ?? ""
            return status.isEmpty ? L("ui.device_settings_section.connecting_to_the_calling_service") : status
        }
        if busy { return L("ui.device_settings_section.a_call_is_already_in_progress") }
        return L("ui.device_settings_section.calls_the_teams_echo_bot_it")
    }
}

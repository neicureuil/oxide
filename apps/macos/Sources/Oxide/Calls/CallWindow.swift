import AVFoundation
import AppKit
import LiveKitWebRTC
import SwiftUI
import OxideCore

/// Window of the pre-join screen and the call, open while the core reports either.
@MainActor
final class CallWindow: NSObject, NSWindowDelegate {
    private var window: NSWindow?
    private weak var store: Store?

    func sync(_ part: CallPart?, store: Store) {
        self.store = store
        let visible = part?.prejoin != nil || part?.active != nil
        if visible { show(store) } else { close() }
        if part?.active?.phase == .incoming { window?.orderFrontRegardless() }
        window?.title = part?.active?.title ?? part?.prejoin?.title ?? L("ui.call_window.call")
    }

    private func show(_ store: Store) {
        if let window {
            if !window.isVisible { window.makeKeyAndOrderFront(nil) }
            return
        }
        let window = NSWindow(
            contentRect: NSRect(x: 0, y: 0, width: 880, height: 600),
            styleMask: [.titled, .closable, .resizable, .miniaturizable],
            backing: .buffered, defer: false
        )
        window.minSize = NSSize(width: 560, height: 420)
        window.isReleasedWhenClosed = false
        window.delegate = self
        window.contentView = NSHostingView(rootView: CallWindowView().environment(store))
        window.center()
        window.makeKeyAndOrderFront(nil)
        self.window = window
    }

    private func close() {
        guard let window else { return }
        self.window = nil
        window.delegate = nil
        window.close()
    }

    /// Closing the window cancels the pre-join, leaves a running call, or dismisses an ended one.
    func windowShouldClose(_ sender: NSWindow) -> Bool {
        guard let store else { return true }
        if store.call?.prejoin != nil {
            store.send(.call(action: .cancelJoin))
        } else if let call = store.call?.active {
            store.send(.call(action: call.phase == .ended ? .dismiss : .hangup))
            if call.phase != .ended { return false }
        }
        return true
    }
}

struct CallWindowView: View {
    @Environment(Store.self) private var store
    var body: some View {
        Group {
            if let prejoin = store.call?.prejoin {
                PrejoinScreen(prejoin: prejoin)
            } else if let call = store.call?.active, call.isTest, call.phase == .ended {
                TestReportView(call: call)
            } else if let call = store.call?.active, call.phase == .incoming {
                IncomingView(call: call)
            } else if let call = store.call?.active {
                InCallView(call: call)
            } else {
                Color.clear
            }
        }
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .background(Theme.background)
        .font(.figtree(13))
        .foregroundStyle(Theme.text)
    }
}

// MARK: Pre-join

private struct PrejoinScreen: View {
    @Environment(Store.self) private var store
    let prejoin: PrejoinView
    @AppStorage("call.prejoin.camera") private var camera = false
    @AppStorage("call.prejoin.muted") private var muted = false
    @AppStorage(MediaDevicePreferences.microphoneKey) private var microphone = ""
    @AppStorage(MediaDevicePreferences.speakerKey) private var speaker = ""
    @AppStorage(MediaDevicePreferences.cameraKey) private var cameraId = ""
    @State private var devices = MediaDevices()
    @State private var mic = MicrophoneTester()
    @State private var cameraDenied = false

    var body: some View {
        HStack(alignment: .top, spacing: 28) {
            VStack(spacing: 14) {
                ZStack {
                    RoundedRectangle(cornerRadius: 14).fill(Color.black)
                    if camera && !cameraDenied {
                        CameraPreview(device: selectedCamera).clipShape(RoundedRectangle(cornerRadius: 14))
                    } else {
                        VStack(spacing: 10) {
                            Avatar(name: store.shell?.displayName ?? L("ui.call_window.you"), id: store.shell?.me ?? "me", size: 72)
                            Text(cameraDenied ? L("ui.call_window.camera_access_denied") : L("ui.call_window.camera_is_off"))
                                .font(.figtree(12)).foregroundStyle(.white.opacity(0.7))
                        }
                    }
                }
                .aspectRatio(16 / 9, contentMode: .fit)
                HStack(spacing: 10) {
                    ToggleButton(on: !muted, symbol: muted ? "mic.slash.fill" : "mic.fill",
                                 label: muted ? L("ui.call_window.mic_is_off") : L("ui.call_window.mic_is_on")) { muted.toggle() }
                    ToggleButton(on: camera, symbol: camera ? "video.fill" : "video.slash.fill",
                                 label: camera ? L("ui.call_window.camera_is_on") : L("ui.call_window.camera_is_off")) {
                        if camera { camera = false } else {
                            Task {
                                cameraDenied = !(await MediaPermissions.request(.video))
                                camera = !cameraDenied
                            }
                        }
                    }
                    LevelBar(tester: mic).frame(width: 90, height: 6).opacity(muted ? 0.3 : 1)
                        .help(L("ui.call_window.mic_level"))
                }
            }
            .frame(maxWidth: 480)

            VStack(alignment: .leading, spacing: 16) {
                Text(prejoin.title).font(.figtree(22, .semibold)).lineLimit(3)
                Muted(L("ui.call_window.choose_your_audio_and_video_settings"), size: 13)
                devicePicker(L("ui.call_window.microphone"), symbol: "mic", selection: $microphone,
                             options: devices.microphones.map { ($0.uid, $0.name) })
                    .onChange(of: microphone) { _, _ in restartMeter() }
                devicePicker(L("ui.call_window.speaker"), symbol: "speaker.wave.2", selection: $speaker,
                             options: devices.speakers.map { ($0.uid, $0.name) })
                devicePicker(L("ui.call_window.camera"), symbol: "video", selection: $cameraId,
                             options: devices.cameras.map { ($0.uniqueID, $0.name) })
                if let error = store.call?.error {
                    Text(error).font(.figtree(12)).foregroundStyle(Theme.danger)
                }
                Spacer()
                if prejoin.otherDevices > 0 {
                    otherDeviceChoice
                } else {
                    HStack {
                        Button(L("ui.call_window.cancel")) { store.send(.call(action: .cancelJoin)) }
                            .buttonStyle(OutlineButtonStyle())
                            .keyboardShortcut(.cancelAction)
                        Spacer()
                        Button(L("ui.call_window.join_now")) { join(transfer: false) }
                            .buttonStyle(PrimaryButtonStyle())
                            .keyboardShortcut(.defaultAction)
                    }
                }
            }
            .frame(width: 300)
        }
        .padding(28)
        .task { await mic.start() }
        .onDisappear { mic.stop() }
    }

    /// Already in the meeting elsewhere: join on this device too, or move the meeting here.
    private var otherDeviceChoice: some View {
        VStack(alignment: .leading, spacing: 10) {
            Label(L("ui.call_window.you_re_already_in_this_meeting"), systemImage: "laptopcomputer.and.iphone")
                .font(.figtree(13, .semibold))
            Button { join(transfer: true) } label: {
                Text(L("ui.call_window.transfer_to_this_device")).frame(maxWidth: .infinity)
            }
            .buttonStyle(PrimaryButtonStyle())
            .keyboardShortcut(.defaultAction)
            .help(L("ui.call_window.the_other_device_leaves_the_meeting"))
            Button {
                mic.stop()
                store.send(.call(action: .confirmJoin(video: false, muted: true, speaker: false, transfer: false)))
            } label: {
                Text(L("ui.call_window.add_this_device")).frame(maxWidth: .infinity)
            }
            .buttonStyle(OutlineButtonStyle())
            .help(L("ui.call_window.join_without_mic_camera_or_audio"))
            Button(L("ui.call_window.cancel")) { store.send(.call(action: .cancelJoin)) }
                .buttonStyle(.link)
                .keyboardShortcut(.cancelAction)
        }
    }

    private func join(transfer: Bool) {
        mic.stop()
        store.send(.call(action: .confirmJoin(video: camera && !cameraDenied, muted: muted, speaker: true, transfer: transfer)))
    }

    private var selectedCamera: AVCaptureDevice? { devices.camera(preferred: cameraId) }

    private func restartMeter() {
        mic.stop()
        Task { await mic.start() }
    }

    private func devicePicker(_ title: String, symbol: String, selection: Binding<String>,
                              options: [(id: String, name: String)]) -> some View {
        VStack(alignment: .leading, spacing: 4) {
            Label(title, systemImage: symbol).font(.figtree(12, .semibold)).foregroundStyle(Theme.muted)
            Picker(title, selection: selection) {
                Text(L("ui.call_window.system_default")).tag("")
                ForEach(options, id: \.id) { Text($0.name).tag($0.id) }
                if !selection.wrappedValue.isEmpty && !options.contains(where: { $0.id == selection.wrappedValue }) {
                    Text(L("ui.call_window.device_not_connected")).tag(selection.wrappedValue)
                }
            }
            .labelsHidden()
        }
    }
}

// MARK: Test call report

private struct TestReportView: View {
    @Environment(Store.self) private var store
    let call: ActiveCallView

    private var report: TestCallReport {
        let speaker = MediaDevicePreferences.resolvedSpeaker()
        return TestCallReport.make(store.media.diagnostics, TestCallReport.Context(
            microphones: CoreAudioDevices.devices(input: true).count,
            speakers: CoreAudioDevices.devices(input: false).count,
            mutedAtEnd: call.muted,
            outputMuted: speaker.map(CoreAudioDevices.isMuted) ?? false,
            hasCamera: MediaDevicePreferences.resolvedCamera() != nil
        ))
    }

    var body: some View {
        let report = report
        VStack(alignment: .leading, spacing: 18) {
            HStack(spacing: 12) {
                Image(systemName: report.isClean ? "checkmark.seal.fill" : "exclamationmark.triangle.fill")
                    .font(.system(size: 30))
                    .foregroundStyle(report.isClean ? Color.green : Theme.danger)
                VStack(alignment: .leading, spacing: 3) {
                    Text(L("ui.call_window.test_call_results")).font(.figtree(20, .semibold))
                    Muted(report.isClean ? L("ui.call_window.no_problems_detected_did_you_hear") : call.status, size: 13)
                }
            }
            VStack(spacing: 0) {
                row(L("ui.call_window.microphone"), "mic.fill", report.microphone)
                Divider()
                row(L("ui.call_window.speaker"), "speaker.wave.2.fill", report.speaker)
                Divider()
                row(L("ui.call_window.camera"), "video.fill", report.camera)
                Divider()
                row(L("ui.call_window.network"), "network", report.network)
            }
            .card()
            Muted(L("ui.call_window.this_report_lists_the_problems_detected"), size: 11)
            Spacer()
            HStack {
                Button(L("ui.call_window.audio_and_video_settings")) {
                    store.openSettings(.audioVideo)
                    store.send(.call(action: .dismiss))
                }
                .buttonStyle(OutlineButtonStyle())
                Spacer()
                Button(L("ui.call_window.test_again")) {
                    store.send(.call(action: .dismiss))
                    store.send(.call(action: .testCall(botMri: TestCall.botOverride)))
                }
                .buttonStyle(OutlineButtonStyle())
                Button(L("ui.call_window.close")) { store.send(.call(action: .dismiss)) }
                    .buttonStyle(PrimaryButtonStyle())
                    .keyboardShortcut(.defaultAction)
            }
        }
        .padding(28)
    }

    private func row(_ title: String, _ symbol: String, _ problems: [String]) -> some View {
        HStack(alignment: .top, spacing: 12) {
            Image(systemName: symbol).frame(width: 20).foregroundStyle(Theme.muted)
            Text(title).font(.figtree(14, .semibold)).frame(width: 110, alignment: .leading)
            VStack(alignment: .leading, spacing: 4) {
                if problems.isEmpty {
                    Label("OK", systemImage: "checkmark.circle.fill").foregroundStyle(Color.green)
                } else {
                    ForEach(problems, id: \.self) { problem in
                        Label(problem, systemImage: "xmark.circle.fill").foregroundStyle(Theme.danger)
                    }
                }
            }
            .font(.figtree(13))
            Spacer()
        }
        .padding(14)
    }
}

// MARK: Incoming

private struct IncomingView: View {
    @Environment(Store.self) private var store
    let call: ActiveCallView
    private func send(_ action: CallAction) { store.send(.call(action: action)) }

    var body: some View {
        VStack(spacing: 18) {
            Spacer()
            Avatar(name: call.title, id: call.id, size: 96)
            Text(call.title).font(.figtree(24, .semibold)).multilineTextAlignment(.center)
            Muted(call.offersVideo ? L("ui.call_window.incoming_video_call") : L("ui.call_window.incoming_audio_call"), size: 14)
            if !call.canAccept { ProgressView().controlSize(.small) }
            HStack(spacing: 16) {
                Button { send(.decline) } label: {
                    answer(L("ui.call_window.decline"), systemImage: "phone.down.fill", color: Theme.danger)
                }
                .buttonStyle(.plain).pointerCursor()
                .keyboardShortcut(.cancelAction)
                Button { send(.accept(video: false)) } label: {
                    answer(L("ui.call_window.accept"), systemImage: "phone.fill", color: .green)
                }
                .buttonStyle(.plain).pointerCursor()
                .keyboardShortcut(.defaultAction)
                .disabled(!call.canAccept)
                if call.offersVideo {
                    Button { send(.accept(video: true)) } label: {
                        answer(L("ui.call_window.with_video"), systemImage: "video.fill", color: .green)
                    }
                    .buttonStyle(.plain).pointerCursor()
                    .disabled(!call.canAccept)
                }
            }
            Spacer()
        }
        .frame(maxWidth: .infinity)
        .padding(28)
    }

    private func answer(_ title: String, systemImage: String, color: Color) -> some View {
        Label(title, systemImage: systemImage)
            .font(.figtree(14, .semibold)).foregroundStyle(.white)
            .padding(.horizontal, 18).frame(height: 42)
            .background(color, in: RoundedRectangle(cornerRadius: 12))
    }
}

// MARK: In call

private struct InCallView: View {
    @Environment(Store.self) private var store
    let call: ActiveCallView
    @State private var showRoster = true
    @State private var pickingScreen = false
    @State private var layout = CallLayoutState()
    private func send(_ action: CallAction) { store.send(.call(action: action)) }

    var body: some View {
        VStack(spacing: 0) {
            header
            if call.phase == .lobby {
                Banner(symbol: "hourglass", text: call.status, tint: Theme.accentSoft)
            }
            if call.isTest && call.phase == .connected {
                Banner(symbol: "waveform", text: L("ui.call_window.speak_after_the_beep_the_echo"), tint: Theme.accentSoft)
            }
            if let notice = call.notice, call.phase != .ended {
                Banner(symbol: "record.circle", text: notice, tint: Theme.errorBackground)
            }
            if call.sharingScreen && call.phase == .connected {
                Banner(symbol: "rectangle.on.rectangle", text: L("ui.call_window.you_re_sharing_your_screen"), tint: Theme.accentSoft)
            }
            if store.media.reconnecting && call.phase == .connected {
                Banner(symbol: "wifi.exclamationmark", text: L("ui.call_window.reconnecting"), tint: Theme.errorBackground)
            }
            if let error = store.call?.error {
                Banner(symbol: "exclamationmark.triangle", text: error, tint: Theme.errorBackground)
            }
            HStack(spacing: 0) {
                stage.padding(16)
                if showRoster && call.isMeeting && call.phase != .ended {
                    Divider()
                    RosterPanel(participants: call.participants).frame(width: 240)
                }
            }
            if call.phase == .ended {
                endBar
            } else {
                controls
            }
        }
        .sheet(isPresented: $pickingScreen) {
            ScreenSharePicker(share: { choice in
                pickingScreen = false
                store.media.chosenScreen = choice
                send(.setScreenShare(on: true))
            }, cancel: { pickingScreen = false })
        }
    }

    private var header: some View {
        HStack(spacing: 10) {
            VStack(alignment: .leading, spacing: 2) {
                Text(call.title).font(.figtree(16, .semibold)).lineLimit(1)
                HStack(spacing: 6) {
                    if [.preparing, .connecting, .ringing].contains(call.phase) { ProgressView().controlSize(.mini) }
                    if let start = call.connectedAtMs, call.phase == .connected {
                        TimelineView(.periodic(from: .now, by: 1)) { context in
                            Text(CallClock.elapsed(since: start, now: context.date)).monospacedDigit()
                        }
                        .font(.figtree(12)).foregroundStyle(Theme.muted)
                        Text("·").foregroundStyle(Theme.subtle)
                    }
                    Text(call.status).font(.figtree(12)).foregroundStyle(Theme.muted).lineLimit(1)
                }
            }
            Spacer()
        }
        .padding(.horizontal, 18).padding(.vertical, 12)
        .background(Theme.surface)
    }

    /// Outgoing call not answered yet: the other side rings, or is about to.
    private var calling: Bool {
        !call.isMeeting && !call.isTest && [.preparing, .connecting, .ringing].contains(call.phase)
    }

    @ViewBuilder private var stage: some View {
        if calling {
            ZStack(alignment: .bottomTrailing) {
                VStack(spacing: 18) {
                    RingingAvatar(name: call.title, id: call.id, ringing: call.phase == .ringing)
                    Text(call.title).font(.figtree(22, .semibold)).multilineTextAlignment(.center)
                    Muted(call.phase == .ringing ? L("ui.call_window.ringing_waiting_for_an_answer") : L("ui.call_window.calling"), size: 14)
                }
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .background(Theme.conversation, in: RoundedRectangle(cornerRadius: 14))
                if call.video && call.camera {
                    VideoTile(track: store.media.localVideo, mirrored: true, placeholder: L("ui.call_window.you"))
                        .frame(width: 200, height: 112)
                        .shadow(radius: 6)
                        .padding(12)
                }
            }
        } else if call.phase == .connected {
            CallStage(call: call, layout: layout)
        } else if call.video && call.phase != .ended {
            VideoTile(track: store.media.localVideo, mirrored: true, placeholder: placeholder)
        } else {
            VStack(spacing: 14) {
                Avatar(name: call.title, id: call.id, size: 96, shape: call.isMeeting ? .square : .round)
                Text(placeholder).font(.figtree(14)).foregroundStyle(Theme.muted)
            }
            .frame(maxWidth: .infinity, maxHeight: .infinity)
            .background(Theme.conversation, in: RoundedRectangle(cornerRadius: 14))
        }
    }

    private var placeholder: String {
        if call.phase == .ended { return call.status }
        if call.isMeeting {
            let count = call.participants.filter { !$0.inLobby }.count
            return count == 0 ? L("ui.call_window.no_one_else_is_here_yet")
                : Lp("ui.call_window.other_participants", count)
        }
        return call.title
    }

    private var controls: some View {
        HStack(spacing: 12) {
            Spacer()
            ToggleButton(on: !call.muted, symbol: call.muted ? "mic.slash.fill" : "mic.fill",
                         label: call.muted ? L("ui.call_window.unmute") : L("ui.call_window.mute")) {
                send(.setMuted(muted: !call.muted))
            }
            .keyboardShortcut("m", modifiers: [.command, .shift])
            ToggleButton(on: call.camera, symbol: call.camera ? "video.fill" : "video.slash.fill",
                         label: call.camera ? L("ui.call_window.turn_camera_off") : L("ui.call_window.turn_camera_on")) {
                send(.setCamera(on: !call.camera))
            }
            .keyboardShortcut("o", modifiers: [.command, .shift])
            .disabled(call.phase != .connected && !call.video)
            ToggleButton(on: call.sharingScreen,
                         symbol: call.sharingScreen ? "rectangle.on.rectangle.slash" : "rectangle.on.rectangle",
                         label: call.sharingScreen ? L("ui.call_window.stop_sharing") : L("ui.call_window.share_screen")) {
                if call.sharingScreen { send(.setScreenShare(on: false)) } else { pickingScreen = true }
            }
            .keyboardShortcut("e", modifiers: [.command, .shift])
            .disabled(call.phase != .connected)
            ToggleButton(on: call.speaker, symbol: call.speaker ? "speaker.wave.2.fill" : "speaker.slash.fill",
                         label: call.speaker ? L("ui.call_window.mute_speaker") : L("ui.call_window.unmute_speaker")) {
                send(.setSpeaker(on: !call.speaker))
            }
            if call.phase == .connected {
                Menu {
                    ForEach(CallLayout.allCases) { option in
                        Button {
                            layout.pick(option)
                        } label: {
                            Label(option.title, systemImage: layout.current == option ? "checkmark" : option.symbol)
                        }
                    }
                    if let pinned = layout.pinned {
                        Divider()
                        Button(L("ui.call_window.unpin") + (call.participants.first { $0.id == pinned }?.name ?? "")) { layout.pinned = nil }
                    }
                } label: {
                    Image(systemName: layout.current.symbol)
                        .font(.system(size: 16))
                        .frame(width: 44, height: 38)
                        .foregroundStyle(Theme.text)
                        .background(Theme.input, in: RoundedRectangle(cornerRadius: 10))
                }
                .menuStyle(.button)
                .buttonStyle(.plain)
                .menuIndicator(.hidden)
                .fixedSize()
                .pointerCursor()
                .help(L("ui.call_window.layout"))
                .accessibilityLabel(L("ui.call_window.layout_2") + layout.current.title)
            }
            if call.isMeeting {
                ToggleButton(on: showRoster, symbol: "person.2.fill", label: L("ui.call_window.participants")) { showRoster.toggle() }
            }
            let bannersMuted = store.notifications?.mutedInCall ?? false
            ToggleButton(on: !bannersMuted, symbol: bannersMuted ? "bell.slash.fill" : "bell.fill",
                         label: bannersMuted ? L("ui.call_window.show_notifications_again") : L("ui.call_window.hide_notifications_during_this_call")) {
                store.send(.notifications(action: .muteDuringThisCall(muted: !bannersMuted)))
            }
            Button { send(.hangup) } label: {
                Label(calling ? L("ui.call_window.cancel_call") : L("ui.call_window.leave"), systemImage: "phone.down.fill")
                    .font(.figtree(13, .semibold)).foregroundStyle(.white)
                    .padding(.horizontal, 16).frame(height: 38)
                    .background(Theme.danger, in: RoundedRectangle(cornerRadius: 10))
            }
            .buttonStyle(.plain).pointerCursor()
            .disabled(call.phase == .ending)
            Spacer()
        }
        .padding(.vertical, 12)
        .background(Theme.surface)
    }

    private var endBar: some View {
        HStack {
            Spacer()
            Button(L("ui.call_window.close")) { send(.dismiss) }
                .buttonStyle(PrimaryButtonStyle())
                .keyboardShortcut(.defaultAction)
            Spacer()
        }
        .padding(.vertical, 14)
        .background(Theme.surface)
    }
}

private struct RosterPanel: View {
    let participants: [CallParticipantView]
    var body: some View {
        let waiting = participants.filter(\.inLobby)
        let present = participants.filter { !$0.inLobby }
        List {
            Section(L("ui.call_window.in_the_meeting", ["value": present.count + 1])) {
                Label(L("ui.call_window.you"), systemImage: "person.fill")
                ForEach(present, id: \.id) { participant in
                    HStack {
                        Text(participant.name).lineLimit(1)
                        Spacer()
                        if let role = CallClock.role(participant.role) { Muted(role, size: 11) }
                    }
                }
            }
            if !waiting.isEmpty {
                Section(L("ui.call_window.lobby", ["waitingCount": waiting.count])) {
                    ForEach(waiting, id: \.id) { Text($0.name).foregroundStyle(Theme.muted) }
                }
            }
        }
        .listStyle(.sidebar)
    }
}

private struct Banner: View {
    let symbol: String
    let text: String
    let tint: Color
    var body: some View {
        HStack(spacing: 8) {
            Image(systemName: symbol)
            Text(text).textSelection(.enabled)
            Spacer()
        }
        .font(.figtree(12))
        .padding(.horizontal, 16).padding(.vertical, 8)
        .background(tint)
    }
}

private struct ToggleButton: View {
    let on: Bool
    let symbol: String
    let label: String
    let action: () -> Void
    var body: some View {
        Button(action: action) {
            Image(systemName: symbol)
                .font(.system(size: 16))
                .frame(width: 44, height: 38)
                .foregroundStyle(on ? Theme.text : .white)
                .background(on ? Theme.input : Theme.muted, in: RoundedRectangle(cornerRadius: 10))
        }
        .buttonStyle(.plain).pointerCursor()
        .help(label)
        .accessibilityLabel(label)
    }
}

/// Reads the level itself, so only the bar is redrawn at the meter's rate.
private struct LevelBar: View {
    let tester: MicrophoneTester
    var body: some View {
        GeometryReader { geometry in
            ZStack(alignment: .leading) {
                Capsule().fill(Theme.input)
                Capsule().fill(Theme.accent).frame(width: geometry.size.width * CGFloat(tester.level))
            }
        }
    }
}

/// Labels of the call window, kept pure for tests.
enum CallClock {
    static func elapsed(since startMs: Int64, now: Date) -> String {
        let seconds = max(0, Int(now.timeIntervalSince1970) - Int(startMs / 1000))
        let h = seconds / 3600, m = (seconds % 3600) / 60, s = seconds % 60
        return h > 0 ? String(format: "%d:%02d:%02d", h, m, s) : String(format: "%02d:%02d", m, s)
    }

    static func role(_ role: String) -> String? {
        switch role {
        case "organizer": return L("ui.call_window.organizer")
        case "presenter": return L("ui.call_window.presenter")
        case "attendee": return L("ui.call_window.attendee")
        default: return nil
        }
    }
}

/// The callee's avatar with waves going out while their side rings.
private struct RingingAvatar: View {
    let name: String
    let id: String
    let ringing: Bool
    @State private var pulse = false

    var body: some View {
        ZStack {
            if ringing {
                ForEach(0..<2) { wave in
                    Circle()
                        .stroke(Theme.accent.opacity(0.5), lineWidth: 2)
                        .frame(width: 96, height: 96)
                        .scaleEffect(pulse ? 1.9 : 1)
                        .opacity(pulse ? 0 : 1)
                        .animation(.easeOut(duration: 1.6).repeatForever(autoreverses: false).delay(Double(wave) * 0.8), value: pulse)
                }
            }
            Avatar(name: name, id: id, size: 96)
        }
        .frame(width: 190, height: 190)
        .onAppear { pulse = ringing }
        .onChange(of: ringing) { _, now in pulse = now }
    }
}

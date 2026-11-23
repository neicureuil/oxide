import SwiftUI
import OxideCore

/// Experimental native calling: place a 1:1 call or join a meeting, then control it.
struct CallPanel: View {
    @Environment(Store.self) private var store
    @State private var target = ""
    private func send(_ action: CallAction) { store.send(.call(action: action)) }
    private var isMeeting: Bool { target.contains("teams.") && target.hasPrefix("https://") }

    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack {
                Label(L("ui.call_panel.native_call_experimental"), systemImage: "phone.connection").font(.figtree(14, .semibold))
                Spacer()
                if let status = store.calls?.realtime, !status.isEmpty { Muted(status, size: 11) }
            }
            if let call = store.call?.active {
                ActiveCallBox(call: call)
            } else {
                HStack(spacing: 8) {
                    TextField(L("ui.call_panel.8_orgid_or_teams_meeting_link"), text: $target)
                        .textFieldStyle(.roundedBorder)
                        .onSubmit(start)
                    if isMeeting {
                        Button(L("ui.call_panel.join")) { send(.joinMeeting(url: target, video: false)) }
                            .buttonStyle(PrimaryButtonStyle())
                        Button(L("ui.call_panel.with_video")) { send(.joinMeeting(url: target, video: true)) }
                            .buttonStyle(OutlineButtonStyle())
                    } else {
                        Button(L("ui.call_panel.audio_call")) { start() }.buttonStyle(PrimaryButtonStyle())
                        Button(L("ui.call_panel.video")) { send(.start(mri: target, video: true)) }.buttonStyle(OutlineButtonStyle())
                    }
                }
                .disabled(target.trimmingCharacters(in: .whitespaces).isEmpty)
            }
            if let error = store.call?.error {
                Text(error).font(.figtree(12)).foregroundStyle(Theme.danger).textSelection(.enabled)
            }
            if store.isDemo && store.call?.active == nil {
                HStack {
                    Button(L("ui.call_panel.simulate_incoming_call")) { send(.simulateIncoming(video: false)) }
                    Button(L("ui.call_panel.video_2")) { send(.simulateIncoming(video: true)) }
                }
                .buttonStyle(.link)
            }
        }
        .card(padding: 16)
    }

    private func start() {
        if isMeeting { send(.joinMeeting(url: target, video: false)) } else { send(.start(mri: target, video: false)) }
    }
}

private struct ActiveCallBox: View {
    @Environment(Store.self) private var store
    let call: ActiveCallView
    private func send(_ action: CallAction) { store.send(.call(action: action)) }

    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(spacing: 12) {
                Avatar(name: call.title, id: call.id, size: 40)
                VStack(alignment: .leading, spacing: 3) {
                    Text(call.title).font(.figtree(15, .semibold))
                    HStack(spacing: 6) {
                        if call.phase == .preparing || call.phase == .connecting || call.phase == .ringing {
                            ProgressView().controlSize(.mini)
                        }
                        Text(call.status).font(.figtree(12)).foregroundStyle(Theme.muted)
                    }
                }
                Spacer()
                if call.phase == .ended {
                    Button(L("ui.call_panel.close")) { send(.dismiss) }.buttonStyle(OutlineButtonStyle())
                } else {
                    IconButton(symbol: call.muted ? "mic.slash.fill" : "mic.fill",
                               help: call.muted ? L("ui.call_panel.unmute") : L("ui.call_panel.mute")) {
                        send(.setMuted(muted: !call.muted))
                    }
                    Button { send(.hangup) } label: {
                        Image(systemName: "phone.down.fill").foregroundStyle(.white)
                            .frame(width: 44, height: 32)
                            .background(Theme.danger, in: RoundedRectangle(cornerRadius: 8))
                    }
                    .buttonStyle(.plain).pointerCursor()
                    .accessibilityLabel(L("ui.call_panel.hang_up"))
                }
            }
            if call.video && call.phase != .ended {
                HStack(spacing: 8) {
                    VideoTile(track: store.media.remoteVideo, mirrored: false, placeholder: call.title)
                    VideoTile(track: store.media.localVideo, mirrored: true, placeholder: L("ui.call_panel.you"))
                        .frame(width: 160)
                }
                .frame(height: 180)
            }
            if !call.participants.isEmpty {
                Muted(L("ui.call_panel.participant_s", ["participantsCount": call.participants.count]) + call.participants.map(\.name).joined(separator: ", "))
                    .lineLimit(2)
            }
        }
    }
}

struct VideoTile: View {
    let track: VideoFeed?
    let mirrored: Bool
    let placeholder: String
    var fit = false
    var body: some View {
        ZStack {
            RoundedRectangle(cornerRadius: 10).fill(Color.black)
            if let track {
                VideoView(track: track, mirrored: mirrored, fit: fit).clipShape(RoundedRectangle(cornerRadius: 10))
            } else {
                Text(placeholder).font(.figtree(12)).foregroundStyle(.white.opacity(0.7))
            }
        }
    }
}

/// A WebRTC video track through `AVSampleBufferDisplayLayer`: the prebuilt framework has no macOS video view.
struct VideoView: NSViewRepresentable {
    let track: VideoFeed
    let mirrored: Bool
    var fit = false

    func makeNSView(context: Context) -> VideoLayerView {
        let view = VideoLayerView(mirrored: mirrored, fit: fit)
        view.attach(track)
        return view
    }
    func updateNSView(_ view: VideoLayerView, context: Context) {
        view.attach(track)
    }
    static func dismantleNSView(_ view: VideoLayerView, coordinator: ()) {
        view.attach(nil)
    }
}

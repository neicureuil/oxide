import SwiftUI
import OxideCore

/// How the call window arranges people and a shared screen; without a share, `sharing` and `focus` enlarge the speaker or pinned person.
enum CallLayout: String, CaseIterable, Identifiable {
    case gallery, speaker, sharing, focus

    var id: String { rawValue }

    var title: String {
        switch self {
        case .gallery: L("ui.call_stage.gallery")
        case .speaker: L("ui.call_stage.speaker")
        case .sharing: L("ui.call_stage.content_and_column")
        case .focus: L("ui.call_stage.full_frame")
        }
    }

    var symbol: String {
        switch self {
        case .gallery: "square.grid.2x2"
        case .speaker: "rectangle.bottomthird.inset.filled"
        case .sharing: "rectangle.righthalf.inset.filled"
        case .focus: "rectangle.inset.filled"
        }
    }
}

/// The remembered layout pick; a screen share switches to `sharing` until it ends or another layout is picked.
@MainActor
@Observable
final class CallLayoutState {
    private static let key = "call.layout"
    private(set) var base: CallLayout = CallLayout(rawValue: UserDefaults.standard.string(forKey: key) ?? "") ?? .gallery
    private var whileSharing: CallLayout?
    /// Participant kept in the big area instead of whoever speaks.
    var pinned: String?

    var current: CallLayout { whileSharing ?? base }

    func pick(_ layout: CallLayout) {
        base = layout
        whileSharing = nil
        UserDefaults.standard.set(layout.rawValue, forKey: Self.key)
    }

    func screenShared(_ shared: Bool) {
        whileSharing = shared ? .sharing : nil
    }

    func togglePin(_ id: String) {
        pinned = pinned == id ? nil : id
    }
}

/// One tile: a person (camera or avatar) or the shared screen.
struct StageTile: Identifiable, Equatable {
    let id: String
    let name: String
    var muted = false
    var speaking = false
    var video: VideoFeed?
    var mirrored = false
    var isMe = false
    var isScreen = false

    static func == (a: StageTile, b: StageTile) -> Bool {
        a.id == b.id && a.name == b.name && a.muted == b.muted && a.speaking == b.speaking
            && a.video === b.video && a.isScreen == b.isScreen
    }
}

/// The call's people and shared screen, arranged by the chosen layout.
struct CallStage: View {
    @Environment(Store.self) private var store
    let call: ActiveCallView
    let layout: CallLayoutState
    /// Who spoke last for long enough (1.5 s), so the big area does not jump on every "hm".
    @State private var activeSpeaker: String?

    // MARK: Tiles

    private var me: StageTile {
        StageTile(id: store.myId, name: L("ui.call_stage.you"), muted: call.muted,
                  speaking: !call.muted && store.media.localSpeaking,
                  video: call.camera ? store.media.localVideo : nil, mirrored: true, isMe: true)
    }

    private var others: [StageTile] {
        let heard = store.media.speakingSources
        let people = call.participants.filter { !$0.inLobby }
        return people.map { p in
            StageTile(id: p.id, name: p.name, muted: p.muted,
                      speaking: !p.muted && !heard.isDisjoint(with: p.audioSources),
                      video: camera(of: p.id, alone: people.count == 1))
        }
    }

    /// The participant's camera line, or the only remote video of a direct 1:1 call.
    private func camera(of id: String, alone: Bool) -> VideoFeed? {
        if let view = call.videos.first(where: { !$0.sharing && $0.participantId == id }) {
            return store.media.remoteVideos[view.mid]
        }
        return !call.isMeeting && alone && call.videos.isEmpty ? store.media.remoteVideo : nil
    }

    private var screen: StageTile? {
        guard let view = call.videos.first(where: \.sharing), let track = store.media.remoteVideos[view.mid] else { return nil }
        let name = call.participants.first { $0.id == view.participantId }?.name ?? L("ui.call_stage.a_participant")
        return StageTile(id: "screen:\(view.mid)", name: L("ui.call_stage.is_sharing_their_screen", ["name": name]), video: track, isScreen: true)
    }

    /// The person for the big area: pinned, else the last lasting speaker, else someone else.
    private func focus(in people: [StageTile]) -> StageTile? {
        let others = people.filter { !$0.isMe }
        return others.first { $0.id == layout.pinned }
            ?? others.first { $0.id == activeSpeaker }
            ?? others.first { $0.video != nil }
            ?? others.first
            ?? people.first
    }

    // MARK: Layouts

    var body: some View {
        let me = me
        let others = others
        let people = [me] + others
        let screen = screen
        let speakingNow = others.filter(\.speaking).map(\.id)
        Group {
            switch layout.current {
            case .gallery:
                grid((screen.map { [$0] } ?? []) + people)
            case .speaker:
                let main = focus(in: people)
                strip(main: main, rest: (screen.map { [$0] } ?? []) + people.filter { $0.id != main?.id }, vertical: false)
            case .sharing:
                let main = screen ?? focus(in: people)
                strip(main: main, rest: ordered(people.filter { $0.id != main?.id }), vertical: true)
            case .focus:
                let main = screen ?? focus(in: people)
                let corner = screen != nil ? focus(in: people) : (main?.isMe == true ? nil : me)
                ZStack(alignment: .bottomTrailing) {
                    if let main { tile(main) }
                    if let corner {
                        tile(corner, small: true).frame(width: 200, height: 120).shadow(radius: 6).padding(12)
                    }
                }
            }
        }
        .overlay(alignment: .bottom) {
            if others.isEmpty && screen == nil && call.isMeeting {
                Muted(L("ui.call_stage.no_one_else_is_here_yet"), size: 13).padding(.bottom, 14)
            }
        }
        .onChange(of: screen != nil, initial: true) { _, shared in layout.screenShared(shared) }
        .task(id: speakingNow) {
            guard let first = speakingNow.first else { return }
            try? await Task.sleep(for: .milliseconds(1500))
            guard !Task.isCancelled else { return }
            activeSpeaker = speakingNow.contains(activeSpeaker ?? "") ? activeSpeaker : first
        }
    }

    /// Speaking people first, so they stay in view in a short column.
    private func ordered(_ tiles: [StageTile]) -> [StageTile] {
        tiles.filter(\.speaking) + tiles.filter { !$0.speaking }
    }

    private func grid(_ tiles: [StageTile]) -> some View {
        GeometryReader { geo in
            let columns = max(1, Int(Double(tiles.count).squareRoot().rounded(.up)))
            let rows = max(1, (tiles.count + columns - 1) / columns)
            let spacing: CGFloat = 10
            let height = (geo.size.height - spacing * CGFloat(rows - 1)) / CGFloat(rows)
            VStack(spacing: spacing) {
                ForEach(0..<rows, id: \.self) { row in
                    HStack(spacing: spacing) {
                        ForEach(tiles[(row * columns)..<min(tiles.count, (row + 1) * columns)]) { tile in
                            self.tile(tile).frame(maxWidth: .infinity)
                        }
                    }
                    .frame(height: max(80, height))
                }
            }
        }
    }

    /// A big tile and the others in a bottom row (`vertical: false`) or a right column.
    private func strip(main: StageTile?, rest: [StageTile], vertical: Bool) -> some View {
        let layout = vertical ? AnyLayout(HStackLayout(spacing: 10)) : AnyLayout(VStackLayout(spacing: 10))
        return layout {
            if let main { tile(main) }
            if !rest.isEmpty {
                ScrollView(vertical ? .vertical : .horizontal) {
                    let inner = vertical ? AnyLayout(VStackLayout(spacing: 8)) : AnyLayout(HStackLayout(spacing: 8))
                    inner {
                        ForEach(rest) { tile in
                            self.tile(tile, small: true).frame(width: vertical ? 200 : 180, height: vertical ? 120 : 110)
                        }
                    }
                }
                .scrollIndicators(.never)
                .frame(width: vertical ? 200 : nil, height: vertical ? nil : 110)
            }
        }
    }

    // MARK: Tile and its actions

    /// Compared on its data, so a speaking change redraws only the tiles it touches.
    private func tile(_ tile: StageTile, small: Bool = false) -> some View {
        StageTileView(tile: tile, small: small, layout: layout).equatable()
    }
}

private struct StageTileView: View, Equatable {
    let tile: StageTile
    let small: Bool
    let layout: CallLayoutState

    static func == (a: StageTileView, b: StageTileView) -> Bool {
        a.tile == b.tile && a.tile.mirrored == b.tile.mirrored && a.tile.isMe == b.tile.isMe
            && a.small == b.small && a.layout === b.layout
    }

    var body: some View {
        Group {
            if tile.isScreen, let video = tile.video {
                ZStack(alignment: .topLeading) {
                    VideoTile(feed: video, mirrored: false, placeholder: L("ui.call_stage.screen_sharing"), fit: true)
                    Label(tile.name, systemImage: "rectangle.on.rectangle")
                        .font(.figtree(small ? 11 : 12, .semibold)).lineLimit(1)
                        .padding(.horizontal, 8).padding(.vertical, 4)
                        .background(.thinMaterial, in: Capsule())
                        .padding(8)
                }
            } else {
                ParticipantTile(name: tile.name, id: tile.id, muted: tile.muted, speaking: tile.speaking,
                                video: tile.video, mirrored: tile.mirrored, pinned: layout.pinned == tile.id)
            }
        }
        .contentShape(Rectangle())
        .onTapGesture(count: 2) { enlarge(tile) }
        .contextMenu {
            if tile.isScreen {
                Button(layout.current == .focus ? L("ui.call_stage.show_participants") : L("ui.call_stage.full_frame")) { enlarge(tile) }
            } else {
                Button(L("ui.call_stage.enlarge")) { enlarge(tile) }
                if !tile.isMe {
                    Button(layout.pinned == tile.id ? L("ui.call_stage.unpin") : L("ui.call_stage.pin")) { layout.togglePin(tile.id) }
                }
            }
        }
    }

    /// Double-click: the shared screen toggles `sharing` / `focus`; a person goes big in `speaker`.
    private func enlarge(_ tile: StageTile) {
        if tile.isScreen {
            layout.pick(layout.current == .focus ? .sharing : .focus)
        } else {
            if !tile.isMe { layout.pinned = tile.id }
            layout.pick(.speaker)
        }
    }
}

/// A person: camera, or avatar with a ring pulsing while they speak.
struct ParticipantTile: View {
    let name: String
    let id: String
    let muted: Bool
    let speaking: Bool
    var video: VideoFeed?
    var mirrored = false
    var pinned = false
    @State private var pulse = false

    var body: some View {
        GeometryReader { geo in
            let avatar = max(28, min(80, min(geo.size.width, geo.size.height) * 0.45))
            ZStack(alignment: .bottomLeading) {
                RoundedRectangle(cornerRadius: 12).fill(Theme.conversation)
                if let video {
                    VideoView(feed: video, mirrored: mirrored).clipShape(RoundedRectangle(cornerRadius: 12))
                } else {
                    ZStack {
                        if speaking {
                            Circle()
                                .stroke(Theme.accent, lineWidth: 3)
                                .frame(width: avatar + 8, height: avatar + 8)
                                .scaleEffect(pulse ? 1.18 : 1)
                                .opacity(pulse ? 0.35 : 0.9)
                                .animation(.easeInOut(duration: 0.6).repeatForever(autoreverses: true), value: pulse)
                                .onAppear { pulse = true }
                                .onDisappear { pulse = false }
                        }
                        Avatar(name: name, id: id, size: avatar)
                    }
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                }
                HStack(spacing: 5) {
                    if pinned { Image(systemName: "pin.fill").font(.system(size: 10)).foregroundStyle(Theme.accent) }
                    if muted { Image(systemName: "mic.slash.fill").font(.system(size: 10)).foregroundStyle(Theme.muted) }
                    Text(name).font(.figtree(12, .semibold)).lineLimit(1)
                }
                .padding(.horizontal, 8).padding(.vertical, 4)
                .background(.thinMaterial, in: Capsule())
                .padding(8)
            }
        }
        .overlay(RoundedRectangle(cornerRadius: 12).strokeBorder(speaking ? Theme.accent : Theme.border, lineWidth: speaking ? 2 : 1))
        .animation(.easeOut(duration: 0.15), value: speaking)
        .accessibilityElement(children: .combine)
        .accessibilityLabel(name + (speaking ? L("ui.call_stage.speaking") : "") + (muted ? L("ui.call_stage.muted") : "") + (pinned ? L("ui.call_stage.pinned") : ""))
    }
}

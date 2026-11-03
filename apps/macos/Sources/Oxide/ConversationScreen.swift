import SwiftUI
import OxideCore

private let callsURL = "https://teams.microsoft.com/v2/#/calls"

/// What the conversation screen needs from the shell, so other shell changes leave it alone.
struct ConversationChrome: Equatable {
    let tab: ConversationTab
    let replyOpen: Bool
    let readOnly: Bool
}

struct ConversationScreen: View, Equatable {
    @Environment(Store.self) private var store
    let chrome: ConversationChrome
    let view: ConversationView
    @Namespace private var tabSpace
    @State private var leaving: ChatRow?

    static func == (a: ConversationScreen, b: ConversationScreen) -> Bool {
        a.chrome == b.chrome && a.view == b.view
    }

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider().overlay(Theme.border)
            switch chrome.tab {
            case .files:
                if view.isChannel {
                    FileBrowserScreen(embedded: true)
                } else {
                    SharedScreen()
                }
            case .members:
                MembersScreen()
            case .messages:
                HStack(spacing: 0) {
                    VStack(spacing: 0) {
                        if view.isChannel {
                            Feed(rows: \.postFeed)
                        } else {
                            Feed(rows: \.chatFeed)
                            ChatTyping()
                        }
                        if chrome.replyOpen {
                            Button(L("ui.conversation_screen.new_post")) { store.send(.closeReply) }
                                .buttonStyle(OutlineButtonStyle())
                                .padding(.vertical, 12)
                                .padding(.horizontal, 24)
                                .frame(maxWidth: .infinity, alignment: .leading)
                        } else {
                            Composer(readOnly: chrome.readOnly, replyOpen: chrome.replyOpen, isChannel: view.isChannel)
                        }
                    }
                    if let thread = view.thread {
                        HStack(spacing: 0) {
                            Divider().overlay(Theme.border)
                            ThreadPanel(chrome: chrome, thread: thread)
                                .frame(width: 360)
                        }
                        .transition(.move(edge: .trailing).combined(with: .opacity))
                    }
                }
                .animation(Motion.standard, value: view.thread?.post)
            }
        }
        .animation(Motion.standard, value: chrome.tab)
    }

    private var header: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 12) {
                Avatar(name: view.title, id: view.target.id, size: 36, shape: view.avatarShape, presence: view.presence)
                VStack(alignment: .leading, spacing: 2) {
                    Text(view.title).font(.figtree(17, .semibold)).lineLimit(1)
                    Muted(view.subtitle).lineLimit(1)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                CallButtons(view: view)
                Button {
                    store.send(.setTab(tab: .members))
                } label: {
                    Label(view.memberCount.map(String.init) ?? "", systemImage: "person.2")
                }
                .buttonStyle(OutlineButtonStyle())
                .help("Participants")
                Menu {
                    if view.isChannel, let team = view.target.team {
                        ChannelNotificationMenu(team: team, channel: view.target.id, muted: view.muted)
                    } else {
                        ChatNotificationMenu(id: view.target.id, muted: view.muted)
                    }
                } label: {
                    Image(systemName: view.muted ? "bell.slash" : "bell").frame(width: 28, height: 28)
                }
                .menuStyle(.borderlessButton).menuIndicator(.hidden).fixedSize().pointerCursor()
                .help("Notifications")
                if let chat = view.chat {
                    Menu {
                        ChatActions(row: chat, onLeave: { leaving = $0 })
                    } label: {
                        Image(systemName: "ellipsis").frame(width: 28, height: 28)
                    }
                    .menuStyle(.borderlessButton).menuIndicator(.hidden).fixedSize().pointerCursor()
                    .help(L("ui.conversation_screen.more_actions"))
                }
            }
            .frame(height: 60)
            HStack(spacing: 20) {
                tab(view.isChannel ? L("ui.conversation_screen.posts") : L("ui.conversation_screen.chat"), .messages)
                // Chats show Teams' "Partagé" tab; channels their library.
                tab(view.isChannel ? L("ui.conversation_screen.files") : L("ui.conversation_screen.shared"), .files)
                tab("Participants", .members)
            }
        }
        .padding(.horizontal, 20)
        .background(Theme.surface)
        .confirmationDialog(L("ui.chat_admin.leave_question", ["title": leaving?.title ?? ""]),
                            isPresented: Binding(get: { leaving != nil }, set: { if !$0 { leaving = nil } })) {
            Button(L("ui.chat_admin.leave"), role: .destructive) {
                if let row = leaving { store.send(.chatAdmin(action: .leave(chatId: row.id))) }
                leaving = nil
            }
        } message: {
            Text(L("ui.chat_admin.leave_explanation"))
        }
    }

    private func tab(_ name: String, _ value: ConversationTab) -> some View {
        let active = chrome.tab == value
        return Button {
            store.send(.setTab(tab: value))
        } label: {
            VStack(spacing: 8) {
                Text(name)
                    .font(.figtree(13, active ? .semibold : .regular))
                    .foregroundStyle(active ? Theme.text : Theme.muted)
                    .padding(.top, 8)
                ZStack {
                    Rectangle().fill(.clear)
                    if active {
                        Capsule().fill(Theme.accent).matchedGeometryEffect(id: "underline", in: tabSpace)
                    }
                }
                .frame(height: 2)
            }
            .fixedSize()
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain).pointerCursor()
        .accessibilityAddTraits(active ? .isSelected : [])
    }
}

/// Each feed reads its own rows, so a new message redraws only its table.
private struct Feed: View {
    @Environment(Store.self) private var store
    let rows: KeyPath<Store, FeedRows>
    @State private var controller = FeedController()
    var body: some View {
        FeedTable(rows: store[keyPath: rows], controller: controller)
            .overlay(alignment: .bottom) {
                NewMessagesPill(controller: controller)
                    .animation(Motion.standard, value: controller.atBottom)
            }
    }
}

private struct ChatTyping: View {
    @Environment(Store.self) private var store
    var body: some View { TypingIndicator(text: store.typing) }
}

private struct ThreadPanel: View {
    @Environment(Store.self) private var store
    let chrome: ConversationChrome
    let thread: ThreadView
    var body: some View {
        VStack(spacing: 0) {
            VStack(alignment: .leading, spacing: 12) {
                HStack {
                    Text(L("ui.conversation_screen.thread")).font(.figtree(15, .semibold))
                        .frame(maxWidth: .infinity, alignment: .leading)
                    if let following = thread.following {
                        IconButton(symbol: following ? "bell.fill" : "bell",
                                   help: following ? L("ui.conversation_screen.unfollow_this_thread") : L("ui.conversation_screen.follow_this_thread")) {
                            store.send(.notifications(action: .setPostFollow(post: thread.post, follow: !following)))
                        }
                    }
                    IconButton(symbol: "xmark", help: L("ui.conversation_screen.close_thread")) { store.send(.closeReply) }
                }
                if !thread.rootPreview.isEmpty || !thread.rootSender.isEmpty {
                    HStack(alignment: .top, spacing: 8) {
                        Avatar(name: thread.rootSender, id: thread.rootSenderId, size: 24, shape: thread.rootSenderAvatarShape)
                        VStack(alignment: .leading, spacing: 2) {
                            (Text(thread.rootSender).font(.figtree(12, .semibold))
                                + Text(thread.rootTime.isEmpty ? "" : " · \(thread.rootTime)")
                                    .font(.figtree(11)).foregroundColor(Theme.muted))
                                .lineLimit(1)
                            Muted(thread.rootPreview).lineLimit(4)
                        }
                    }
                    .padding(10)
                    .frame(maxWidth: .infinity, alignment: .leading)
                    .background(Theme.input, in: RoundedRectangle(cornerRadius: 8))
                }
            }
            .padding(16)
            Divider().overlay(Theme.border)
            Feed(rows: \.threadFeed)
            Composer(readOnly: chrome.readOnly, replyOpen: chrome.replyOpen, isChannel: true)
        }
        .background(Theme.surface)
    }
}

/// Join (meeting chats) or call (other chats) natively when enabled, else in Teams Web.
private struct CallButtons: View {
    @Environment(Store.self) private var store
    let view: ConversationView
    private var busy: Bool { store.inCall }

    var body: some View {
        if view.isMeeting {
            Button { store.send(.call(action: .prepareChatMeeting)) } label: {
                Label(L("ui.conversation_screen.join"), systemImage: "video")
            }
            .buttonStyle(PrimaryButtonStyle())
            .disabled(busy)
            .help(L("ui.conversation_screen.join_the_meeting_in_oxide"))
        } else if view.canCall {
            Button { store.send(.call(action: .callChat(chatId: nil, video: true))) } label: {
                Label(L("ui.conversation_screen.video_call"), systemImage: "video")
            }
            .buttonStyle(PrimaryButtonStyle())
            .disabled(busy)
            .help((view.memberCount ?? 0) > 2 ? L("ui.conversation_screen.group_video_call") : L("ui.conversation_screen.video_call"))
            Button { store.send(.call(action: .callChat(chatId: nil, video: false))) } label: {
                Image(systemName: "phone")
            }
            .buttonStyle(OutlineButtonStyle())
            .disabled(busy)
            .help(L("ui.conversation_screen.audio_call"))
        } else {
            Button { store.send(.openUrl(url: callsURL)) } label: {
                Label(L("ui.conversation_screen.meet"), systemImage: "video")
            }
            .buttonStyle(PrimaryButtonStyle())
            .help(L("ui.conversation_screen.meet_in_teams_on_the_web"))
            Button { store.send(.openUrl(url: callsURL)) } label: {
                Image(systemName: "phone")
            }
            .buttonStyle(OutlineButtonStyle())
            .help(L("ui.conversation_screen.audio_call_in_teams_on_the"))
        }
    }
}

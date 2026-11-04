import SwiftUI
import OxideCore

/// Takes only the shell fields it shows, so selection and conversation changes leave it alone.
struct SidebarView: View {
    @Environment(Store.self) private var store
    let nav: Nav
    let filter: Filter
    let unread: UInt32
    let activityUnread: UInt32
    let loading: Bool
    let searching: Bool
    /// Skeleton rows instead of the lists while the rail switches screens.
    var placeholder = false
    @Namespace private var chipSpace

    private var title: String {
        switch nav {
        case .teams: L("ui.sidebar_view.teams")
        case .activity: L("ui.sidebar_view.activity")
        default: L("ui.sidebar_view.chat")
        }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 0) {
            HStack(spacing: 4) {
                Text(title).font(.figtree(20, .semibold)).frame(maxWidth: .infinity, alignment: .leading)
                if nav == .teams {
                    TeamsAddMenu()
                    RefreshButton(help: L("ui.sidebar_view.refresh_teams"), spinning: loading) { store.send(.refresh) }
                } else {
                    IconButton(symbol: "line.3.horizontal.decrease", help: L("ui.sidebar_view.unread")) {
                        store.send(.setFilter(filter: filter == .unread ? .all : .unread))
                    }
                    if nav == .activity {
                        IconButton(symbol: "checkmark.circle", help: L("ui.sidebar_view.mark_all_as_read")) {
                            store.send(.notifications(action: .markAllActivityRead))
                        }
                        .disabled(activityUnread == 0)
                        RefreshButton(help: L("ui.sidebar_view.refresh_notifications"), spinning: store.sidebar?.activityLoading == true) {
                            store.send(.refresh)
                        }
                    } else {
                        Button {
                            store.send(.newChat)
                        } label: {
                            Image(systemName: "square.and.pencil").font(.system(size: 16)).frame(width: 34, height: 34)
                        }
                        .buttonStyle(GhostButtonStyle(accent: true))
                        .help(L("ui.sidebar_view.new_chat"))
                    }
                }
            }
            .padding(EdgeInsets(top: 16, leading: 16, bottom: 10, trailing: 12))
            if placeholder {
                SkeletonList(rows: 8).padding(EdgeInsets(top: 8, leading: 8, bottom: 0, trailing: 8))
                Spacer(minLength: 0)
            } else if let sidebar = store.sidebar {
                if nav == .teams {
                    TeamsList(searching: searching, loading: loading, sidebar: sidebar)
                } else {
                    chips
                    if nav == .activity {
                        ActivityList(unreadOnly: filter == .unread, sidebar: sidebar)
                    } else {
                        ChatList(chats: sidebar.chats, more: sidebar.moreChats, loading: loading).equatable()
                    }
                }
            }
        }
        .background(Theme.surface)
    }

    private var chips: some View {
        HStack(spacing: 6) {
            chip(L("ui.sidebar_view.all"), .all)
            chip(L("ui.sidebar_view.unread") + " · \(nav == .activity ? activityUnread : unread)", .unread)
            if nav != .activity { chip(L("ui.sidebar_view.meetings"), .meetings) }
        }
        .padding(EdgeInsets(top: 0, leading: 16, bottom: 12, trailing: 12))
        .animation(Motion.select, value: filter)
    }
    private func chip(_ title: String, _ chip: Filter) -> some View {
        let active = filter == chip
        return Button {
            store.send(.setFilter(filter: chip))
        } label: {
            Text(title)
                .font(.figtree(12, active ? .semibold : .regular))
                .foregroundStyle(active ? Theme.accentText : Theme.textSoft)
                .padding(.horizontal, 12)
                .frame(height: 28)
                .background {
                    if active {
                        Capsule().fill(Theme.accentSoft).matchedGeometryEffect(id: "chip", in: chipSpace)
                    } else {
                        Capsule().fill(Theme.input)
                    }
                }
        }
        .buttonStyle(.plain).pointerCursor()
        .accessibilityAddTraits(active ? .isSelected : [])
    }
}

private struct ChatList: View, Equatable {
    @Environment(Store.self) private var store
    let chats: [ChatRow]
    let more: Bool
    let loading: Bool
    /// ↑ and ↓ switch chats while the list has the focus, which a click on a chat gives it.
    @FocusState private var focused: Bool
    /// One dialog for the list rather than one per row.
    @State private var leaving: ChatRow?
    static func == (a: ChatList, b: ChatList) -> Bool {
        a.chats == b.chats && a.more == b.more && a.loading == b.loading
    }
    var body: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(alignment: .leading, spacing: 0) {
                    let pinned = chats.filter(\.pinned)
                    let recent = chats.filter { !$0.pinned }
                    if !pinned.isEmpty {
                        header(L("ui.sidebar_view.favorites"), top: 8)
                        ForEach(pinned, id: \.listId) { ChatRowView(row: $0, focused: focused, onSelect: { focused = true }, onLeave: { leaving = $0 }).equatable() }
                    }
                    if !recent.isEmpty {
                        header(L("ui.sidebar_view.recent"), top: pinned.isEmpty ? 8 : 14)
                        ForEach(recent, id: \.listId) { ChatRowView(row: $0, focused: focused, onSelect: { focused = true }, onLeave: { leaving = $0 }).equatable() }
                    }
                    if chats.isEmpty {
                        if loading {
                            SkeletonList(rows: 8).padding(.top, 8)
                                .accessibilityElement().accessibilityLabel(L("ui.sidebar_view.loading_chats"))
                        } else {
                            Muted(L("ui.sidebar_view.no_chats_to_show")).padding(20)
                        }
                    }
                    if more {
                        Button(L("ui.sidebar_view.more_chats")) { store.send(.moreChats) }
                            .buttonStyle(GhostButtonStyle())
                            .font(.figtree(12))
                            .padding(8)
                            .disabled(loading)
                    }
                }
                .padding(EdgeInsets(top: 0, leading: 8, bottom: 12, trailing: 8))
            }
            .focusable()
            .focused($focused)
            .focusEffectDisabled()
            .onKeyPress(.upArrow) { move(-1) }
            .onKeyPress(.downArrow) { move(1) }
            .onChange(of: chats.first(where: \.selected)?.listId) { _, id in
                if focused, let id { proxy.scrollTo(id) }
            }
            .accessibilityLabel(L("ui.sidebar_view.chats"))
            .accessibilityHint(L("ui.sidebar_view.up_and_down_arrows_to_switch"))
        }
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
    private func move(_ offset: Int) -> KeyPress.Result {
        store.selectAdjacentChat(offset)
        return .handled
    }
    private func header(_ title: String, top: CGFloat) -> some View {
        Text(title)
            .font(.figtree(11, .semibold))
            .foregroundStyle(Theme.subtle)
            .padding(EdgeInsets(top: top, leading: 8, bottom: 6, trailing: 8))
    }
}

private extension ChatRow {
    /// Differs between Favorites and Recent: a row moved by pinning is built anew, else it keeps its
    /// hover, which loses its exit, and the context menu it was first given.
    var listId: String { (pinned ? "pinned:" : "recent:") + id }
}

/// Compared on its data only: the closures are new on every pass and would defeat diffing.
private struct ChatRowView: View, Equatable {
    @Environment(Store.self) private var store
    let row: ChatRow
    /// The list has the keyboard focus: the selected row shows it.
    let focused: Bool
    let onSelect: () -> Void
    /// Asks the list to confirm leaving this chat.
    let onLeave: (ChatRow) -> Void
    static func == (a: ChatRowView, b: ChatRowView) -> Bool {
        a.row == b.row && a.focused == b.focused
    }
    @State private var hovered = false
    var body: some View {
        Button {
            onSelect()
            store.send(.select(target: Target(team: nil, id: row.id)))
        } label: {
            HStack(spacing: 12) {
                Avatar(name: row.title, id: row.id, size: 40, shape: row.avatarShape, presence: row.presence)
                VStack(alignment: .leading, spacing: 2) {
                    HStack(spacing: 8) {
                        Text(row.title)
                            .font(.figtree(14, row.unread ? .semibold : .medium))
                            .lineLimit(1)
                            .frame(maxWidth: .infinity, alignment: .leading)
                        if row.hidden {
                            Image(systemName: "eye.slash").font(.system(size: 11)).foregroundStyle(Theme.subtle)
                                .accessibilityLabel(L("ui.chat_admin.hidden"))
                        }
                        if row.muted {
                            Image(systemName: "bell.slash").font(.system(size: 11)).foregroundStyle(Theme.subtle)
                                .accessibilityLabel(L("ui.sidebar_view.notifications_muted"))
                        }
                        Text(row.time).font(.figtree(12)).foregroundStyle(Theme.subtle)
                    }
                    HStack(spacing: 8) {
                        Text(row.preview)
                            .font(.figtree(13))
                            .foregroundStyle(row.unread ? Theme.text : Theme.muted)
                            .lineLimit(1)
                            .frame(maxWidth: .infinity, alignment: .leading)
                        if row.unread {
                            Circle().fill(Theme.accent).frame(width: 10, height: 10)
                                .transition(.pop)
                                .accessibilityLabel(L("ui.sidebar_view.unread_2"))
                        }
                    }
                    .animation(Motion.pop, value: row.unread)
                }
            }
            .padding(.horizontal, 10)
            .padding(.vertical, 8)
            .background(
                row.selected ? Theme.accentSoft : hovered ? Theme.input : .clear,
                in: RoundedRectangle(cornerRadius: 8)
            )
            .overlay(
                RoundedRectangle(cornerRadius: 8)
                    .strokeBorder(row.selected && focused ? Theme.accent : .clear, lineWidth: 1.5)
            )
            .animation(Motion.hover, value: hovered)
            .animation(Motion.hover, value: row.selected)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain).pointerCursor()
        .onHover { hovered = $0 }
        .help(row.title)
        .id(row.listId)
        .accessibilityAddTraits(row.selected ? .isSelected : [])
        .contextMenu { ChatActions(row: row, onLeave: onLeave) }
    }
}

/// What can be done to a chat, from its row in the list and from its header.
struct ChatActions: View {
    @Environment(Store.self) private var store
    let row: ChatRow
    /// Asks for confirmation before leaving: a menu cannot hold the dialog itself.
    let onLeave: (ChatRow) -> Void
    var body: some View {
        Button(row.pinned ? L("ui.chat_admin.unpin") : L("ui.chat_admin.pin")) {
            store.send(.chatAdmin(action: .setPinned(chatId: row.id, pinned: !row.pinned)))
        }
        Button(row.unread ? L("ui.chat_admin.mark_as_read") : L("ui.chat_admin.mark_as_unread")) {
            store.send(.chatAdmin(action: row.unread ? .markRead(chatId: row.id) : .markUnread(chatId: row.id)))
        }
        Button(row.hidden ? L("ui.chat_admin.show") : L("ui.chat_admin.hide")) {
            store.send(.chatAdmin(action: row.hidden ? .unhide(chatId: row.id) : .hide(chatId: row.id)))
        }
        ChatNotificationMenu(id: row.id, muted: row.muted)
        if row.canLeave {
            Divider()
            Button(L("ui.chat_admin.leave"), role: .destructive) { onLeave(row) }
        }
    }
}

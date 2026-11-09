import AppKit
import SwiftUI
import OxideCore

struct MainView: View {
    @Environment(Store.self) private var store
    let shell: Shell
    @FocusState private var searchFocused: Bool
    /// The screen built, which trails `shell.nav` briefly: the click animates over light placeholders
    /// before the new screen's first build takes the main thread.
    @State private var built: Nav?
    private var switching: Bool { built.map { $0 != shell.nav } ?? false }
    /// Chat and Teams both show a conversation, which updates in place faster than a rebuild.
    private var replacingContent: Bool {
        let conversations: [Nav] = [.chat, .teams]
        return switching && !(built.map(conversations.contains) == true && conversations.contains(shell.nav))
    }

    var body: some View {
        VStack(spacing: 0) {
            topBar
            Divider().overlay(Theme.border)
            HStack(spacing: 0) {
                Rail(nav: shell.nav, unread: shell.unread, activityUnread: shell.activityUnread, teamsUnread: shell.teamsUnread).equatable()
                Divider().overlay(Theme.border)
                // These screens bring their own navigation column.
                if ![.calendar, .calls, .files, .settings].contains(shell.nav) {
                    SidebarView(
                        nav: shell.nav, filter: shell.filter, unread: shell.unread, activityUnread: shell.activityUnread,
                        loading: shell.loadingWorkspace,
                        searching: !shell.search.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty,
                        placeholder: switching
                    )
                        .frame(width: 316)
                    Divider().overlay(Theme.border)
                }
                VStack(spacing: 0) {
                    Banners(error: shell.error, notices: shell.notices)
                    Group {
                        if store.search?.open == true {
                            SearchResultsScreen()
                        } else if replacingContent {
                            ProgressView().controlSize(.small).frame(maxWidth: .infinity, maxHeight: .infinity)
                        } else {
                            ContentRouter(shell: shell)
                        }
                    }
                    .transition(.opacity)
                    .animation(Motion.standard, value: store.search?.open == true)
                }
                .animation(Motion.standard, value: shell.error)
                .animation(Motion.standard, value: shell.notices)
                .animation(Motion.standard, value: store.online)
                .frame(maxWidth: .infinity, maxHeight: .infinity)
                .background(Theme.conversation)
            }
        }
        .task(id: shell.nav) {
            guard built != nil else {
                built = shell.nav
                return
            }
            try? await Task.sleep(for: .milliseconds(120))
            guard !Task.isCancelled else { return }
            built = shell.nav
        }
        .sheet(isPresented: Binding(
            get: { store.notifications?.editor != nil },
            set: { if !$0 { store.send(.notifications(action: .closeEditor)) } }
        )) {
            if let editor = store.notifications?.editor { NotificationEditorSheet(editor: editor) }
        }
        // ⌘E searches, as in Teams; ⌘K is left to the composer's link insertion.
        .background {
            Button("") { searchFocused = true }.keyboardShortcut("e").hidden()
        }
    }

    private var topBar: some View {
        HStack(spacing: 16) {
            HStack(spacing: 10) {
                OxideMark().frame(width: 28, height: 26)
                Text("Oxide").font(.figtree(15, .semibold))
            }
            .frame(width: 356, alignment: .leading)
            HStack(spacing: 8) {
                Image(systemName: "magnifyingglass").foregroundStyle(Theme.muted)
                GlobalSearchField(current: shell.search, focused: $searchFocused)
                Text("⌘E")
                    .font(.figtree(11))
                    .foregroundStyle(Theme.muted)
                    .padding(.horizontal, 6)
                    .padding(.vertical, 2)
                    .background(Theme.surface, in: RoundedRectangle(cornerRadius: 4))
            }
            .padding(.horizontal, 12)
            .frame(maxWidth: 560, minHeight: 34)
            .background(Theme.input, in: RoundedRectangle(cornerRadius: 8))
            Spacer()
            RefreshButton(help: L("ui.main_view.refresh"), spinning: shell.loadingWorkspace, side: 36) { store.send(.refresh) }
            AccountButton(name: shell.displayName, id: shell.me, email: shell.accountEmail)
        }
        .padding(.horizontal, 16)
        .frame(height: 52)
        .background(Theme.surface)
    }
}

struct Banners: View {
    @Environment(Store.self) private var store
    let error: String?
    let notices: [String]
    var body: some View {
        OfflineBanner()
        if let error {
            banner(error) {
                store.send(.dismissError)
            }
        }
        if let admin = store.teamsAdmin, let error = admin.error, admin.gallery == nil, admin.joinCode == nil {
            banner(error) {
                store.send(.teamsAdmin(action: .dismissError))
            }
        }
        if let error = store.chatAdmin?.error {
            banner(error) {
                store.send(.chatAdmin(action: .dismissError))
            }
        }
        ForEach(notices, id: \.self) { notice in
            banner(notice, foreground: Theme.text, background: Theme.input) {
                store.send(.dismissNotice(text: notice))
            }
        }
    }
    private func banner(_ text: String, foreground: Color = Theme.errorText, background: Color = Theme.errorBackground,
                        close: @escaping () -> Void) -> some View {
        HStack(spacing: 8) {
            Text(text)
                .font(.figtree(12))
                .foregroundStyle(foreground)
                .textSelection(.enabled)
                .frame(maxWidth: .infinity, alignment: .leading)
            IconButton(symbol: "xmark", help: L("ui.main_view.close"), side: 24, glyph: 11, action: close)
        }
        .padding(.horizontal, 16)
        .padding(.vertical, 6)
        .background(background)
        .transition(.slideDown)
    }
}

/// The top bar's search: filters once typing pauses (150 ms); Escape clears it, then leaves the field.
private struct GlobalSearchField: View {
    @Environment(Store.self) private var store
    /// The core's query: it changes on its own only when cleared.
    let current: String
    var focused: FocusState<Bool>.Binding
    @State private var query = ""

    var body: some View {
        HStack(spacing: 6) {
            TextField(L("ui.main_view.search_chats_channels_and_messages"), text: $query)
                .textFieldStyle(.plain)
                .focused(focused)
                // Return searches every message on the server, as Teams does.
                .onSubmit {
                    send()
                    let filters = store.search?.filters ?? SearchFiltersView.empty
                    store.send(.fullSearch(action: .run(query: query, filters: filters)))
                }
                .help(L("ui.search.press_return"))
                .onExitCommand {
                    if query.isEmpty { focused.wrappedValue = false } else { query = "" }
                }
            if !query.isEmpty {
                IconButton(symbol: "xmark.circle.fill", help: L("ui.main_view.clear_search"), side: 22, glyph: 12) {
                    query = ""
                }
            }
        }
        .task(id: query) {
            guard query != current else { return }
            if !query.isEmpty {
                try? await Task.sleep(for: .milliseconds(150))
                guard !Task.isCancelled else { return }
            }
            send()
        }
        .onAppear { query = current }
        .onChange(of: current) { _, value in
            if value != query && !focused.wrappedValue { query = value }
        }
    }
    private func send() {
        if query != current { store.send(.search(query: query)) }
    }
}

/// Stays while the Mac has no usable network: messages can be written but not sent.
private struct OfflineBanner: View {
    @Environment(Store.self) private var store
    var body: some View {
        if !store.online {
            HStack(spacing: 8) {
                Image(systemName: "wifi.slash").foregroundStyle(Theme.errorText)
                Text(L("ui.main_view.you_re_offline_messages_can_t"))
                    .font(.figtree(12, .medium))
                    .foregroundStyle(Theme.errorText)
                    .frame(maxWidth: .infinity, alignment: .leading)
                Button(L("ui.main_view.try_again")) { store.send(.refresh) }
                    .buttonStyle(GhostButtonStyle())
                    .font(.figtree(12))
            }
            .padding(.horizontal, 16)
            .padding(.vertical, 6)
            .background(Theme.errorBackground)
            .transition(.slideDown)
            .accessibilityElement(children: .combine)
            .accessibilityAddTraits(.isStaticText)
        }
    }
}

/// Takes only what it shows, so the shell's other changes do not redraw it.
struct Rail: View, Equatable {
    @Environment(Store.self) private var store
    let nav: Nav
    let unread: UInt32
    let activityUnread: UInt32
    let teamsUnread: Bool
    /// Highlighted from the click until the core confirms, which may take a full rebuild.
    @State private var pending: Nav?
    @Namespace private var selectionSpace
    private let items = RailCommands.items
    static func == (a: Rail, b: Rail) -> Bool {
        a.nav == b.nav && a.unread == b.unread && a.activityUnread == b.activityUnread
            && a.teamsUnread == b.teamsUnread
    }
    var body: some View {
        VStack(spacing: 4) {
            ForEach(items, id: \.2) { item, symbol, name in
                RailItem(
                    symbol: symbol, name: name,
                    active: (pending ?? nav) == item,
                    badge: item == .activity ? activityUnread : item == .chat ? unread : 0,
                    dot: item == .teams && teamsUnread,
                    selectionSpace: selectionSpace
                ) { go(item) }
            }
            Spacer()
            IconButton(symbol: "gearshape", help: L("ui.main_view.settings")) { go(.settings) }
                .padding(.bottom, 12)
        }
        .padding(.top, 8)
        .frame(width: 72)
        .background(Theme.surface)
        .animation(Motion.select, value: pending ?? nav)
        // Clicks are applied in order: an earlier one landing keeps the latest highlighted.
        .onChange(of: nav) { if nav == pending { pending = nil } }
    }
    private func go(_ target: Nav) {
        pending = target == nav ? nil : target
        store.send(.navigate(nav: target))
    }
}

private struct RailItem: View {
    let symbol: String
    let name: String
    let active: Bool
    let badge: UInt32
    /// Unread without a count, as Teams shows for channels.
    let dot: Bool
    let selectionSpace: Namespace.ID
    let action: () -> Void
    @State private var hovered = false
    /// Bumped when the item becomes active, so only the newly selected icon bounces.
    @State private var bounces = 0
    var body: some View {
        Button(action: action) {
            VStack(spacing: 4) {
                Image(systemName: symbol)
                    .symbolVariant(active ? .fill : .none)
                    .symbolEffect(.bounce, options: .speed(1.5), value: bounces)
                    .font(.system(size: 19))
                    .frame(height: 22)
                    .overlay(alignment: .topTrailing) {
                        if badge > 0 {
                            Text("\(min(badge, 99))")
                                .font(.figtree(10, .semibold))
                                .contentTransition(.numericText(value: Double(badge)))
                                .foregroundStyle(.white)
                                .padding(.horizontal, 4)
                                .frame(minWidth: 16, minHeight: 16)
                                .background(Theme.danger, in: Capsule())
                                .offset(x: 10, y: -6)
                                .transition(.pop)
                        } else if dot {
                            Circle().fill(Theme.danger)
                                .frame(width: 8, height: 8)
                                .offset(x: 6, y: -2)
                                .transition(.pop)
                        }
                    }
                    .animation(Motion.pop, value: badge)
                    .animation(Motion.pop, value: dot)
                Text(name).font(.figtree(11, active ? .semibold : .regular))
            }
            .foregroundStyle(active ? Theme.accentText : hovered ? Theme.text : Theme.muted)
            .frame(width: 60, height: 56)
            .background {
                if active {
                    RoundedRectangle(cornerRadius: 8).fill(Theme.accentSoft)
                        .matchedGeometryEffect(id: "selection", in: selectionSpace)
                } else if hovered {
                    RoundedRectangle(cornerRadius: 8).fill(Theme.input)
                }
            }
            .animation(Motion.hover, value: hovered)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain).pointerCursor()
        .onHover { hovered = $0 }
        .onChange(of: active) { if active && !Motion.reduced { bounces += 1 } }
        .accessibilityLabel(name)
        .accessibilityValue(badge > 0 ? L("ui.sidebar_view.unread") + " · \(badge)" : dot ? L("ui.sidebar_view.unread") : "")
        .accessibilityAddTraits(active ? .isSelected : [])
    }
}

struct ContentRouter: View {
    @Environment(Store.self) private var store
    let shell: Shell
    var body: some View {
        screen
            .transition(.opacity)
            .animation(Motion.standard, value: shell.nav)
            .animation(Motion.standard, value: store.newChat == nil)
    }
    @ViewBuilder private var screen: some View {
        if let dialog = store.newChat {
            NewChatScreen(dialog: dialog)
        } else {
            switch shell.nav {
            case .settings:
                SettingsScreen(shell: shell)
            case .calendar:
                CalendarScreen()
            case .calls:
                CallsScreen()
            case .files:
                FileBrowserScreen(embedded: false)
            case .activity where !(store.sidebar?.activities.contains(where: { $0.selected && $0.hasTarget }) ?? false):
                ActivityOverview(row: store.sidebar?.activities.first(where: \.selected))
            case .teams where shell.selected?.team == nil:
                TeamsOverview(shell: shell)
            default:
                if let conversation = store.conversation {
                    ConversationScreen(
                        chrome: ConversationChrome(tab: shell.tab, replyOpen: shell.replyOpen, readOnly: shell.readOnly, editing: shell.editing != nil),
                        view: conversation
                    )
                    .equatable()
                } else {
                    Welcome(loading: shell.loadingWorkspace)
                }
            }
        }
    }
}

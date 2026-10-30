import SwiftUI
import OxideCore

struct Welcome: View {
    @Environment(Store.self) private var store
    let loading: Bool
    var body: some View {
        VStack(spacing: 16) {
            Image(systemName: "bubble.left").font(.system(size: 48)).foregroundStyle(Theme.accent)
                .symbolEffect(.pulse, isActive: loading && !Motion.reduced)
            Text(L("ui.screens.your_workspace")).font(.figtree(26, .semibold))
            if loading {
                LoadingLabel(text: L("ui.screens.loading_your_conversations"), size: 14)
            } else {
                Muted(L("ui.screens.find_your_private_chats_and_your"), size: 14)
                    .transition(.opacity)
            }
            Button(L("ui.screens.new_chat")) { store.send(.newChat) }.buttonStyle(PrimaryButtonStyle())
        }
        .padding(32)
        .frame(maxWidth: .infinity, maxHeight: .infinity)
        .animation(Motion.standard, value: loading)
    }
}

struct TeamsOverview: View {
    @Environment(Store.self) private var store
    let shell: Shell
    var body: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 16) {
                Text(L("ui.screens.your_teams")).font(.figtree(24, .semibold))
                Muted(L("ui.screens.open_a_channel_to_see_its"), size: 14)
                if let sidebar = store.sidebar {
                    if sidebar.teams.isEmpty {
                        Muted(
                            shell.loadingWorkspace ? L("ui.screens.loading_teams")
                                : sidebar.teamsProblem != nil
                                    ? L("ui.screens.the_teams_service_couldn_t_be")
                                : L("ui.screens.no_teams_available_for_this_account"),
                            size: 14
                        )
                        if let problem = sidebar.teamsProblem {
                            Muted(problem).textSelection(.enabled)
                            Button(L("ui.screens.try_again")) { store.send(.refresh) }.buttonStyle(OutlineButtonStyle())
                        }
                    }
                    ForEach(sidebar.teams, id: \.id) { team in
                        VStack(alignment: .leading, spacing: 10) {
                            Text(team.name).font(.figtree(18, .semibold))
                            if !team.description.isEmpty {
                                Muted(team.description, size: 13)
                            }
                            ForEach(team.channels, id: \.id) { channel in
                                Button(channel.name) {
                                    store.send(.select(target: Target(team: channel.team, id: channel.id)))
                                }
                                .buttonStyle(GhostButtonStyle(accent: true))
                                .font(.figtree(14))
                                .padding(.vertical, 2)
                            }
                        }
                        .frame(maxWidth: .infinity, alignment: .leading)
                        .card(padding: 18)
                    }
                }
            }
            .padding(28)
        }
    }
}

struct MembersScreen: View {
    @Environment(Store.self) private var store
    /// Local filter on name and role, for long channel rosters.
    @State private var filter = ""
    @State private var removing: MemberView?
    var body: some View {
        ScrollView {
            LazyVStack(alignment: .leading, spacing: 16) {
                HStack {
                    Text(L("ui.screens.participants")).font(.figtree(20, .semibold)).frame(maxWidth: .infinity, alignment: .leading)
                    if store.members?.isChannel == true {
                        IconButton(symbol: "arrow.clockwise", help: L("ui.screens.refresh_participants")) {
                            store.send(.channelMembers)
                        }
                    }
                }
                if let view = store.members {
                    if view.canManage { GroupChatControls(view: view) }
                    if let members = view.members {
                        if members.count > 6 {
                            FileSearchField(placeholder: L("ui.screens.filter_participants"), current: filter, delay: .zero) {
                                filter = $0
                            }
                        }
                        let shown = filter.isEmpty ? members : members.filter {
                            $0.name.localizedStandardContains(filter) || $0.role.localizedStandardContains(filter)
                        }
                        Muted(shown.count == members.count
                              ? L("ui.screens.participants_2", ["membersCount": members.count])
                              : L("ui.screens.of_participants", ["shownCount": shown.count, "membersCount": members.count]))
                        ForEach(shown, id: \.id) { member in
                            HStack(spacing: 12) {
                                Avatar(name: member.name, id: member.id, size: 36, presence: member.presence)
                                VStack(alignment: .leading, spacing: 3) {
                                    Text(member.name).font(.figtree(14))
                                    Muted(member.role)
                                }
                                Spacer()
                                if view.canManage && member.id.lowercased() != store.shell?.me.lowercased() {
                                    IconButton(symbol: "person.badge.minus",
                                               help: L("ui.chat_admin.remove_from_chat", ["name": member.name])) {
                                        removing = member
                                    }
                                }
                            }
                            .card(padding: 12)
                        }
                        if members.isEmpty {
                            Muted(L("ui.screens.the_service_returned_no_participants"), size: 13)
                        }
                    }
                    if view.loading {
                        LoadingLabel(text: L("ui.screens.loading_participants"), size: 13)
                    }
                    if let error = view.error {
                        Muted(L("ui.screens.couldn_t_load_participants", ["error": error]), size: 13)
                            .textSelection(.enabled)
                        Button(L("ui.screens.try_again")) { store.send(.channelMembers) }.buttonStyle(OutlineButtonStyle())
                    }
                }
            }
            .padding(24)
        }
        .confirmationDialog(
            L("ui.chat_admin.remove_question", ["name": removing?.name ?? ""]),
            isPresented: Binding(get: { removing != nil }, set: { if !$0 { removing = nil } })
        ) {
            Button(L("ui.chat_admin.remove"), role: .destructive) {
                if let member = removing { store.send(.chatAdmin(action: .removeMember(mri: member.id))) }
                removing = nil
            }
        }
        .sheet(isPresented: Binding(
            get: { store.chatAdmin?.adding != nil },
            set: { if !$0 { store.send(.chatAdmin(action: .cancelAdding)) } }
        )) {
            if let adding = store.chatAdmin?.adding { AddPeopleSheet(adding: adding) }
        }
    }
}

struct NewChatScreen: View {
    @Environment(Store.self) private var store
    let dialog: NewChatView
    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 18) {
                HStack {
                    Text(L("ui.screens.new_chat")).font(.figtree(22, .semibold))
                        .frame(maxWidth: .infinity, alignment: .leading)
                    IconButton(symbol: "xmark", help: L("ui.screens.cancel")) { store.send(.cancelNewChat) }
                }
                Muted(L("ui.screens.search_for_a_person_or_pick"), size: 13)
                HStack(spacing: 8) {
                    SyncedTextField(
                        placeholder: L("ui.screens.name_or_email_address"),
                        value: dialog.query,
                        onChange: { store.send(.peopleQuery(query: $0)) },
                        onSubmit: { store.send(.peopleSearch) }
                    )
                    .textFieldStyle(.roundedBorder)
                    Button(dialog.searching ? L("ui.screens.searching") : L("ui.screens.search")) { store.send(.peopleSearch) }
                        .buttonStyle(OutlineButtonStyle())
                        .disabled(dialog.searching || dialog.query.trimmingCharacters(in: .whitespaces).count < 2)
                }
                if !dialog.chosen.isEmpty {
                    VStack(alignment: .leading, spacing: 8) {
                        Muted(L("ui.screens.to"), size: 11)
                        ForEach(dialog.chosen, id: \.mri) { person in
                            HStack(spacing: 8) {
                                Avatar(name: person.name, id: person.mri, size: 30)
                                Text(person.name).frame(maxWidth: .infinity, alignment: .leading)
                                IconButton(symbol: "xmark", help: L("ui.screens.remove")) {
                                    store.send(.removePerson(mri: person.mri))
                                }
                            }
                        }
                    }
                }
                VStack(alignment: .leading, spacing: 4) {
                    if dialog.searched && dialog.results.isEmpty && !dialog.query.isEmpty {
                        Muted(L("ui.screens.no_people_found_for_this_search"), size: 13)
                    }
                    ForEach(dialog.results, id: \.mri) { person in
                        Button {
                            store.send(.choosePerson(mri: person.mri))
                        } label: {
                            HStack(spacing: 10) {
                                Avatar(name: person.name, id: person.mri, size: 34)
                                VStack(alignment: .leading, spacing: 3) {
                                    Text(person.name).font(.figtree(13, .semibold)).foregroundStyle(Theme.text)
                                    if !person.email.isEmpty { Muted(person.email, size: 11) }
                                }
                                Spacer()
                            }
                            .padding(10)
                            .contentShape(Rectangle())
                        }
                        .buttonStyle(GhostButtonStyle())
                    }
                }
                if dialog.chosen.count > 1 {
                    SyncedTextField(
                        placeholder: L("ui.screens.group_name_optional"),
                        value: dialog.title,
                        onChange: { store.send(.newTitle(title: $0)) }
                    )
                    .textFieldStyle(.roundedBorder)
                }
                HStack {
                    Button(L("ui.screens.cancel")) { store.send(.cancelNewChat) }
                        .buttonStyle(OutlineButtonStyle())
                        .keyboardShortcut(.cancelAction)
                    Spacer()
                    Button(dialog.creating ? L("ui.screens.creating") : L("ui.screens.create_chat")) { store.send(.createChat) }
                        .buttonStyle(PrimaryButtonStyle())
                        .disabled(dialog.chosen.isEmpty || dialog.creating)
                }
            }
            .padding(28)
            .frame(maxWidth: 650)
            .card(radius: 16)
            .padding(28)
            .frame(maxWidth: .infinity)
        }
    }
}

/// The settings sub-pages, opened from the settings home like Notifications.
enum SettingsPage: Hashable {
    case notifications, refresh, appearance, language, audioVideo, outOfOffice, debug
}

struct SettingsScreen: View {
    @Environment(Store.self) private var store
    @AppStorage(Appearance.storageKey) private var appearance = Appearance.system
    @AppStorage(LanguagePreference.storageKey) private var language = LanguagePreference.system
    let shell: Shell
    var body: some View {
        ScrollView {
            Group {
                switch store.settingsPage {
                case .notifications:
                    NotificationSettingsPage(back: back)
                case .refresh:
                    SettingsSubpage(title: L("ui.screens.message_refresh"), back: back) {
                        RefreshSettingsSection()
                    }
                case .appearance:
                    SettingsSubpage(title: L("ui.screens.appearance"), back: back) { AppearanceSettingsSection() }
                case .language:
                    SettingsSubpage(title: L("ui.screens.language"), back: back) {
                        LanguageSettingsSection()
                        SpellingSettingsSection()
                    }
                case .outOfOffice:
                    OutOfOfficePage(back: back)
                case .audioVideo:
                    SettingsSubpage(title: L("ui.screens.audio_and_video"), back: back) {
                        DeviceSettingsSection()
                    }
                case .debug:
                    SettingsSubpage(title: L("ui.debug.title"), back: back) { DebugSettingsSection() }
                case nil:
                    home
                }
            }
            .frame(maxWidth: 760, alignment: .leading)
            .padding(32)
            .frame(maxWidth: .infinity, alignment: .top)
        }
        // Coming back to Settings starts from its home.
        .onDisappear { store.settingsPage = nil }
    }

    private var home: some View {
        VStack(alignment: .leading, spacing: 22) {
            Text(L("ui.screens.your_account")).font(.figtree(24, .semibold))
            HStack(spacing: 16) {
                Avatar(name: shell.displayName, id: shell.me, size: 48)
                VStack(alignment: .leading, spacing: 5) {
                    Text(shell.displayName).font(.figtree(17, .semibold))
                    if !shell.accountEmail.isEmpty {
                        Muted(shell.accountEmail).textSelection(.enabled)
                    }
                    Label(L("ui.screens.microsoft_sign_in_session_saved_in"), systemImage: "lock.fill")
                        .font(.figtree(11)).foregroundStyle(Theme.subtle)
                        .padding(.top, 2)
                }
                Spacer()
                Button(L("ui.screens.sign_out")) { store.send(.signOut) }.buttonStyle(OutlineButtonStyle())
            }
            .card(padding: 20)
            // Two columns: the window's minimum width always leaves room for both.
            Grid(horizontalSpacing: 12, verticalSpacing: 12) {
                GridRow {
                    NotificationSettingsLink { open(.notifications) }
                    RefreshSettingsLink { open(.refresh) }
                }
                GridRow {
                    SettingsLink(symbol: "circle.lefthalf.filled", title: L("ui.screens.appearance"),
                                 summary: appearance.label) { open(.appearance) }
                    SettingsLink(symbol: "globe", title: L("ui.screens.language"),
                                 summary: language.label) { open(.language) }
                }
                GridRow {
                    SettingsLink(symbol: "airplane", title: L("ui.out_of_office.title"),
                                 summary: store.me?.outOfOffice == true ? L("ui.status_menu.automatic_replies_on")
                                     : L("ui.out_of_office.summary")) { open(.outOfOffice) }
                    SettingsLink(symbol: "video", title: L("ui.screens.audio_and_video"),
                                 summary: L("ui.screens.microphone_speaker_camera_test_call")) { open(.audioVideo) }
                }
                GridRow {
                    SettingsLink(symbol: "memorychip", title: L("ui.debug.title"),
                                 summary: L("ui.debug.summary", ["size": MemoryReport.footprintText])) { open(.debug) }
                }
            }
            // The app's signature, quiet at the foot of the page.
            VStack(spacing: 6) {
                OxideMark().frame(width: 28, height: 28)
                Text("Oxide").font(.figtree(13, .semibold)).foregroundStyle(Theme.textSoft)
                Muted(L("ui.screens.version", ["version": displayVersion]), size: 11).textSelection(.enabled)
                UpdateSettingsRow().padding(.top, 4)
            }
            .frame(maxWidth: .infinity)
            .padding(.top, 14)
        }
    }

    /// "0.1.0 (42)": the core's version, plus the bundle's build number when launched from Oxide.app.
    private var displayVersion: String {
        let build = Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String
        return build.map { "\(appVersion()) (\($0))" } ?? appVersion()
    }

    private func open(_ page: SettingsPage) { store.settingsPage = page }
    private func back() { store.settingsPage = nil }
}

/// A settings sub-page: back button and title, then its sections.
struct SettingsSubpage<Content: View>: View {
    let title: String
    let back: () -> Void
    @ViewBuilder let content: Content
    var body: some View {
        VStack(alignment: .leading, spacing: 22) {
            HStack(spacing: 8) {
                IconButton(symbol: "chevron.left", help: L("ui.screens.back_to_settings"), action: back)
                Text(title).font(.figtree(24, .semibold))
            }
            content
        }
    }
}

/// A row of the settings home that opens a sub-page.
struct SettingsLink: View {
    let symbol: String
    let title: String
    let summary: String
    let open: () -> Void
    var body: some View {
        Button(action: open) {
            HStack(spacing: 12) {
                Image(systemName: symbol).font(.system(size: 16)).foregroundStyle(Theme.accent)
                    .frame(width: 22)
                VStack(alignment: .leading, spacing: 3) {
                    Text(title).font(.figtree(14, .semibold))
                    Muted(summary, size: 12)
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                Image(systemName: "chevron.right").foregroundStyle(Theme.subtle)
            }
            // Side by side in the grid, cards share the row's height.
            .frame(maxHeight: .infinity)
            .contentShape(Rectangle())
        }
        .buttonStyle(.plain).pointerCursor()
        .card(padding: 18)
        .accessibilityHint(L("ui.screens.opens_the_page", ["title": title]))
    }
}

/// Settings › Apparence.
struct AppearanceSettingsSection: View {
    @AppStorage(Appearance.storageKey) private var appearance = Appearance.system
    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text(L("ui.screens.theme")).font(.figtree(14, .semibold))
            SegmentedPicker(selection: $appearance, options: Appearance.allCases, label: \.label)
                .accessibilityLabel(L("ui.screens.appearance"))
        }
        .card(padding: 18)
    }
}

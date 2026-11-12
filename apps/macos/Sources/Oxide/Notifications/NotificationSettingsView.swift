import SwiftUI
import OxideCore

/// "Notifications et activité"; the core writes every change to the account (`userPersonalSettings`).
struct NotificationSettingsPage: View {
    @Environment(Store.self) private var store
    let back: () -> Void
    @State private var customPause = false

    var body: some View {
        SettingsSubpage(title: L("ui.notification_settings_view.notifications_and_activity"), back: back) {
            if let view = store.notifications {
                if let error = view.error {
                    Text(error).font(.figtree(12)).foregroundStyle(Theme.danger)
                }
                permissionCard(view)
                pauseCard(view)
                presetCard(view)
                section(L("ui.notification_settings_view.chats"), rows: view.rows.filter { $0.section == .chats }) {
                    Muted(L("ui.notification_settings_view.a_muted_chat_no_longer_shows"), size: 12)
                }
                section(L("ui.notification_settings_view.teams_and_channels"), rows: view.rows.filter { $0.section == .teamsAndChannels }) {
                    Muted(L("ui.notification_settings_view.each_channel_also_has_its_own"), size: 12)
                }
                meetingsCard(view)
                soundsCard(view)
                readReceiptsCard(view)
                emailCard(view)
                if !view.realtime.isEmpty {
                    Label(view.realtime, systemImage: "antenna.radiowaves.left.and.right")
                        .font(.figtree(12)).foregroundStyle(Theme.muted)
                }
            } else {
                ProgressView()
            }
        }
        .sheet(isPresented: $customPause) { PauseUntilSheet() }
    }

    private func send(_ action: NotificationsAction) {
        store.send(.notifications(action: action))
    }

    // MARK: Cards

    private func permissionCard(_ view: NotificationsView) -> some View {
        HStack(spacing: 12) {
            Image(systemName: view.permission == .granted ? "bell.badge" : "bell.slash")
                .font(.system(size: 18)).foregroundStyle(view.permission == .granted ? Theme.accent : Theme.danger)
            VStack(alignment: .leading, spacing: 3) {
                Text(permissionTitle(view.permission)).font(.figtree(14, .semibold))
                Muted(permissionDetail(view.permission), size: 12)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            if view.permission == .granted {
                Button(L("ui.notification_settings_view.test")) { send(.sendTest) }.buttonStyle(OutlineButtonStyle())
            } else {
                Button(L("ui.notification_settings_view.open_system_settings")) { NSWorkspace.shared.open(NotificationPermissionState.settingsURL) }
                    .buttonStyle(OutlineButtonStyle())
            }
        }
        .card(padding: 18)
    }

    private func permissionTitle(_ permission: NotificationPermission) -> String {
        switch permission {
        case .granted: L("ui.notification_settings_view.macos_notifications_allowed")
        case .denied: L("ui.notification_settings_view.macos_notifications_denied")
        case .unknown: L("ui.notification_settings_view.macos_notifications_not_set_up")
        }
    }
    private func permissionDetail(_ permission: NotificationPermission) -> String {
        switch permission {
        case .granted: L("ui.notification_settings_view.banner_style_and_focus_are_set")
        case .denied: L("ui.notification_settings_view.no_banners_can_be_shown_while")
        case .unknown: L("ui.notification_settings_view.run_the_packaged_app_oxide_app")
        }
    }

    private func pauseCard(_ view: NotificationsView) -> some View {
        HStack(spacing: 12) {
            Image(systemName: "moon").font(.system(size: 16)).foregroundStyle(Theme.muted)
            VStack(alignment: .leading, spacing: 3) {
                if let until = view.pausedUntil {
                    Text(L("ui.notification_settings_view.banners_paused_until", ["until": until])).font(.figtree(14, .semibold))
                } else {
                    Text(L("ui.notification_settings_view.pause_banners")).font(.figtree(14, .semibold))
                }
                Muted(L("ui.notification_settings_view.the_activity_feed_badges_and_emails"), size: 12)
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            if view.pausedUntil != nil {
                Button(L("ui.notification_settings_view.resume")) { send(.resume) }.buttonStyle(OutlineButtonStyle())
            } else {
                Menu(L("ui.notification_settings_view.pause")) {
                    Button(L("ui.notification_settings_view.10_minutes")) { send(.pause(minutes: 10)) }
                    Button(L("ui.notification_settings_view.30_minutes")) { send(.pause(minutes: 30)) }
                    Button(L("ui.notification_settings_view.1_hour")) { send(.pause(minutes: 60)) }
                    Button(L("ui.notification_settings_view.until_tomorrow_8_00_am")) { send(.pauseUntilTomorrow) }
                    Divider()
                    Button(L("ui.notification_settings_view.date_and_time")) { customPause = true }
                }
                .fixedSize()
            }
        }
        .card(padding: 18)
    }

    private func presetCard(_ view: NotificationsView) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            Text(L("ui.notification_settings_view.preset")).font(.figtree(16, .semibold))
            Picker(L("ui.notification_settings_view.preset"), selection: Binding(
                get: { view.preset },
                set: { send(.setPreset(preset: $0)) }
            )) {
                Text(L("ui.notification_settings_view.all_activity")).tag(NotificationPreset.allActivity)
                Text(L("ui.notification_settings_view.mentions_and_replies")).tag(NotificationPreset.myAtMentions)
                Text(L("ui.notification_settings_view.custom")).tag(NotificationPreset.custom)
            }
            .pickerStyle(.segmented)
            .labelsHidden()
            Muted(L("ui.notification_settings_view.a_preset_rewrites_every_row_below"), size: 12)
        }
    }

    private func section(_ title: String, rows: [NotificationRowView],
                         @ViewBuilder footer: () -> some View) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            Text(title).font(.figtree(16, .semibold))
            VStack(alignment: .leading, spacing: 12) {
                ForEach(Array(rows.enumerated()), id: \.element.row) { index, row in
                    if index > 0 { Divider() }
                    choiceRow(row)
                }
            }
            .card(padding: 18)
            footer()
        }
    }

    private func choiceRow(_ row: NotificationRowView) -> some View {
        HStack {
            Text(row.title).font(.figtree(13)).frame(maxWidth: .infinity, alignment: .leading)
            Picker(row.title, selection: Binding(
                get: { row.choice },
                set: { send(.setChoice(row: row.row, choice: $0)) }
            )) {
                Text(L("ui.notification_settings_view.banner_and_feed")).tag(NotificationChoice.bannerAndFeed)
                Text(L("ui.notification_settings_view.only_show_in_feed")).tag(NotificationChoice.onlyFeed)
                Text(L("ui.notification_settings_view.off")).tag(NotificationChoice.off)
            }
            .labelsHidden()
            .frame(width: 190)
        }
    }

    private func meetingsCard(_ view: NotificationsView) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            Text(L("ui.notification_settings_view.meetings")).font(.figtree(16, .semibold))
            VStack(alignment: .leading, spacing: 12) {
                ForEach(view.rows.filter { $0.section == .meetings }, id: \.row) { choiceRow($0) }
                Divider()
                HStack {
                    Text(L("ui.notification_settings_view.meeting_chats")).font(.figtree(13)).frame(maxWidth: .infinity, alignment: .leading)
                    Picker(L("ui.notification_settings_view.meeting_chats"), selection: Binding(
                        get: { view.meetingChats },
                        set: { send(.setMeetingChats(choice: $0)) }
                    )) {
                        Text(L("ui.notification_settings_view.always_notify")).tag(MeetingChats.all)
                        Text(L("ui.notification_settings_view.mute_until_i_join_or_send")).tag(MeetingChats.participated)
                        Text(L("ui.notification_settings_view.mute")).tag(MeetingChats.none)
                    }
                    .labelsHidden()
                    .frame(width: 250)
                }
                Divider()
                toggle(L("ui.notification_settings_view.mute_notifications_during_meetings_and_calls"), view.muteDuringMeetings, .muteDuringMeetings)
            }
            .card(padding: 18)
        }
    }

    private func soundsCard(_ view: NotificationsView) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            Text(L("ui.notification_settings_view.sounds_and_preview")).font(.figtree(16, .semibold))
            VStack(alignment: .leading, spacing: 12) {
                toggle(L("ui.notification_settings_view.show_message_preview"), view.showPreview, .showPreview)
                Divider()
                toggle(L("ui.notification_settings_view.play_sound_for_notifications"), view.sounds, .sounds)
                soundPicker(L("ui.notification_settings_view.sound"), selection: view.sound, options: NotificationSounds.standard, urgent: false)
                    .disabled(!view.sounds)
                soundPicker(L("ui.notification_settings_view.urgent_sound"), selection: view.urgentSound, options: NotificationSounds.urgent, urgent: true)
                    .disabled(!view.sounds)
                toggle(L("ui.notification_settings_view.sounds_when_i_m_busy_or"), view.soundsWhenBusy, .soundsWhenBusy)
                    .disabled(!view.sounds)
                toggle(L("ui.notification_settings_view.sounds_for_urgent_and_priority_messages"), view.soundsForUrgent, .soundsForUrgent)
                    .disabled(!view.sounds)
                Divider()
                toggle(L("ui.notification_settings_view.ring_for_incoming_calls"), view.callSounds, .callSounds)
                Divider()
                toggle(L("ui.notification_settings_view.only_urgent_and_priority_notifications"), view.onlyUrgent, .onlyUrgent)
            }
            .card(padding: 18)
        }
    }

    private func readReceiptsCard(_ view: NotificationsView) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            Text(L("ui.notification_settings_view.privacy")).font(.figtree(16, .semibold))
            VStack(alignment: .leading, spacing: 8) {
                Toggle(isOn: Binding(get: { view.readReceipts }, set: { send(.setReadReceipts(on: $0)) })) {
                    Text(L("ui.notification_settings_view.read_receipts")).font(.figtree(13)).frame(maxWidth: .infinity, alignment: .leading)
                }
                .toggleStyle(.switch)
                Muted(L("ui.notification_settings_view.read_receipts_detail"), size: 12)
            }
            .card(padding: 18)
        }
    }

    private func emailCard(_ view: NotificationsView) -> some View {
        VStack(alignment: .leading, spacing: 10) {
            Text(L("ui.notification_settings_view.email")).font(.figtree(16, .semibold))
            HStack {
                Text(L("ui.notification_settings_view.missed_activity_emails")).font(.figtree(13)).frame(maxWidth: .infinity, alignment: .leading)
                Picker(L("ui.notification_settings_view.missed_activity_emails"), selection: Binding(
                    get: { view.emailDelay },
                    set: { send(.setEmailDelay(delay: $0)) }
                )) {
                    Text(L("ui.notification_settings_view.as_soon_as_possible")).tag(EmailDelay.immediately)
                    Text(L("ui.notification_settings_view.every_10_minutes")).tag(EmailDelay.every10Mins)
                    Text(L("ui.notification_settings_view.hourly")).tag(EmailDelay.hourly)
                    Text(L("ui.notification_settings_view.every_8_hours")).tag(EmailDelay.everyEightHours)
                    Text(L("ui.notification_settings_view.daily")).tag(EmailDelay.daily)
                    Text(L("ui.notification_settings_view.never")).tag(EmailDelay.never)
                }
                .labelsHidden()
                .frame(width: 210)
            }
            .card(padding: 18)
        }
    }

    // MARK: Controls

    private func toggle(_ title: String, _ value: Bool, _ toggle: NotificationToggle) -> some View {
        Toggle(isOn: Binding(get: { value }, set: { send(.setToggle(toggle: toggle, on: $0)) })) {
            Text(title).font(.figtree(13)).frame(maxWidth: .infinity, alignment: .leading)
        }
        .toggleStyle(.switch)
    }

    private func soundPicker(_ title: String, selection: NotificationSound, options: [NotificationSound],
                             urgent: Bool) -> some View {
        HStack {
            Text(title).font(.figtree(13)).frame(maxWidth: .infinity, alignment: .leading)
            Picker(title, selection: Binding(
                get: { selection },
                set: { sound in
                    NotificationRouter.shared.play(sound)
                    send(.setSound(urgent: urgent, sound: sound))
                }
            )) {
                ForEach(options, id: \.self) { Text(NotificationSounds.label($0)).tag($0) }
            }
            .labelsHidden()
            .frame(width: 190)
            IconButton(symbol: "speaker.wave.2", help: L("ui.notification_settings_view.play"), side: 28, glyph: 13) {
                NotificationRouter.shared.play(selection)
            }
        }
    }
}

/// Settings entry: opens the notification page.
struct NotificationSettingsLink: View {
    @Environment(Store.self) private var store
    let open: () -> Void
    var body: some View {
        SettingsLink(symbol: "bell", title: L("ui.notification_settings_view.notifications_and_activity"), summary: summary, open: open)
    }
    private var summary: String {
        guard let view = store.notifications else { return L("ui.notification_settings_view.banners_sounds_meetings_email") }
        if let until = view.pausedUntil { return L("ui.notification_settings_view.paused_until", ["until": until]) }
        if view.permission == .denied { return L("ui.notification_settings_view.denied_by_macos") }
        return L("ui.notification_settings_view.banners_sounds_meetings_email")
    }
}

private struct PauseUntilSheet: View {
    @Environment(Store.self) private var store
    @Environment(\.dismiss) private var dismiss
    @State private var date = Date().addingTimeInterval(2 * 3600)
    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            Text(L("ui.notification_settings_view.pause_banners_until")).font(.figtree(16, .semibold))
            DatePicker(L("ui.notification_settings_view.end"), selection: $date, in: Date()..., displayedComponents: [.date, .hourAndMinute])
                .labelsHidden()
            HStack {
                Spacer()
                Button(L("ui.notification_settings_view.cancel")) { dismiss() }.buttonStyle(OutlineButtonStyle())
                Button(L("ui.notification_settings_view.pause")) {
                    store.send(.notifications(action: .pauseUntil(epochMs: Int64(date.timeIntervalSince1970 * 1000))))
                    dismiss()
                }
                .buttonStyle(PrimaryButtonStyle())
            }
        }
        .padding(24)
        .frame(width: 360)
    }
}

/// Mute / settings items of a chat's context menu and header menu.
struct ChatNotificationMenu: View {
    @Environment(Store.self) private var store
    let id: String
    let muted: Bool
    var body: some View {
        Button(muted ? L("ui.notification_settings_view.unmute_notifications") : L("ui.notification_settings_view.mute_notifications")) {
            store.send(.notifications(action: .setChatMuted(conversationId: id, muted: !muted)))
        }
        Button(L("ui.notification_settings_view.notification_settings")) {
            store.send(.notifications(action: .editChat(conversationId: id)))
        }
    }
}

/// Notification items of a channel's menu.
struct ChannelNotificationMenu: View {
    @Environment(Store.self) private var store
    let team: String
    let channel: String
    let muted: Bool
    var body: some View {
        Button(L("ui.notification_settings_view.channel_notifications")) {
            store.send(.notifications(action: .editChannel(team: team, channel: channel)))
        }
        if !muted {
            Button(L("ui.notification_settings_view.turn_off_channel_notifications")) {
                store.send(.notifications(action: .muteChannel(team: team, channel: channel)))
            }
        }
    }
}

/// Sheet of `NotificationsView.editor`: one chat's or one channel's settings.
struct NotificationEditorSheet: View {
    @Environment(Store.self) private var store
    let editor: NotificationEditorView
    @State private var draft: Draft?

    struct Draft: Equatable {
        var muted: Bool
        var everyone: Bool
        var desktop: ChatDesktop
        var posts: ChannelChoice
        var replies: Bool
        var mentions: ChannelChoice
        var followAll: Bool
        init(_ editor: NotificationEditorView) {
            muted = editor.muted
            everyone = editor.everyoneMentions
            desktop = editor.desktop
            posts = editor.allNewPosts
            replies = editor.includeReplies
            mentions = editor.channelMentions
            followAll = editor.followAllThreads
        }
    }

    var body: some View {
        VStack(alignment: .leading, spacing: 16) {
            VStack(alignment: .leading, spacing: 3) {
                Text(L("ui.notification_settings_view.notifications")).font(.figtree(18, .semibold))
                Muted(editor.title, size: 13).lineLimit(1)
            }
            if editor.loading {
                ProgressView().frame(maxWidth: .infinity)
            } else if draft != nil {
                if editor.channel { channelForm } else { chatForm }
            }
            if let error = editor.error {
                Text(error).font(.figtree(12)).foregroundStyle(Theme.danger).fixedSize(horizontal: false, vertical: true)
            }
            HStack {
                if editor.saving { ProgressView().controlSize(.small) }
                Spacer()
                Button(L("ui.notification_settings_view.close")) { close() }.buttonStyle(OutlineButtonStyle())
                Button(L("ui.notification_settings_view.save")) { save() }
                    .buttonStyle(PrimaryButtonStyle())
                    .disabled(draft == nil || draft == Draft(editor) || editor.loading || editor.saving)
            }
        }
        .padding(24)
        .frame(width: 440)
        .onAppear { if !editor.loading { draft = Draft(editor) } }
        .onChange(of: editor.loading) { _, loading in if !loading { draft = Draft(editor) } }
    }

    private var chatForm: some View {
        VStack(alignment: .leading, spacing: 12) {
            Toggle(isOn: bind(\.muted)) {
                VStack(alignment: .leading, spacing: 2) {
                    Text(L("ui.notification_settings_view.mute_chat")).font(.figtree(13))
                    Muted(L("ui.notification_settings_view.mentions_and_urgent_messages_still_show"), size: 11)
                }
            }
            .toggleStyle(.switch)
            Toggle(isOn: bind(\.everyone)) { Text(L("ui.notification_settings_view.everyone_mentions_everyone")).font(.figtree(13)) }
                .toggleStyle(.switch)
                .disabled(draft?.muted == true)
            Picker(L("ui.notification_settings_view.banners"), selection: bind(\.desktop)) {
                Text(L("ui.notification_settings_view.follow_general_settings")).tag(ChatDesktop.followGlobal)
                Text(L("ui.notification_settings_view.always_show")).tag(ChatDesktop.on)
                Text(L("ui.notification_settings_view.never_show")).tag(ChatDesktop.off)
            }
        }
    }

    private var channelForm: some View {
        VStack(alignment: .leading, spacing: 12) {
            Picker(L("ui.notification_settings_view.all_new_posts"), selection: bind(\.posts)) { channelChoices }
            Toggle(isOn: Binding(
                get: { draft?.posts != .off && bind(\.replies).wrappedValue },
                set: { bind(\.replies).wrappedValue = $0 }
            )) { Text(L("ui.notification_settings_view.include_all_replies")).font(.figtree(13)) }
                .toggleStyle(.switch)
                .disabled(draft?.posts == .off)
            Picker(L("ui.notification_settings_view.channel_mentions"), selection: bind(\.mentions)) { channelChoices }
            Divider()
            Toggle(isOn: bind(\.followAll)) {
                VStack(alignment: .leading, spacing: 2) {
                    Text(L("ui.notification_settings_view.follow_all_new_threads")).font(.figtree(13))
                    Muted(L("ui.notification_settings_view.replies_to_every_post_are_notified"), size: 11)
                }
            }
            .toggleStyle(.switch)
        }
    }

    @ViewBuilder private var channelChoices: some View {
        Text(L("ui.notification_settings_view.banner_and_feed")).tag(ChannelChoice.bannerAndFeed)
        Text(L("ui.notification_settings_view.only_show_in_feed")).tag(ChannelChoice.feed)
        Text(L("ui.notification_settings_view.off")).tag(ChannelChoice.off)
    }

    private func bind<T>(_ path: WritableKeyPath<Draft, T>) -> Binding<T> {
        Binding(
            get: { (draft ?? Draft(editor))[keyPath: path] },
            set: { value in
                var next = draft ?? Draft(editor)
                next[keyPath: path] = value
                draft = next
            }
        )
    }

    private func save() {
        guard let draft else { return }
        if editor.channel {
            store.send(.notifications(action: .setChannelSettings(
                allNewPosts: draft.posts, includeReplies: draft.replies,
                channelMentions: draft.mentions, followAllThreads: draft.followAll
            )))
        } else {
            store.send(.notifications(action: .setChatSettings(
                muted: draft.muted, everyoneMentions: draft.everyone, desktop: draft.desktop
            )))
        }
    }

    private func close() {
        store.send(.notifications(action: .closeEditor))
    }
}

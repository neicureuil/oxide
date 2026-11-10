import AppKit
import ImageIO
import Observation
import OxideCore

/// Latest core view-models; each property changes only when the core reports that part, so only its readers redraw.
@MainActor
@Observable
final class Store {
    private(set) var calendar: CalendarView?
    private(set) var calls: CallsView?
    let voicemailPlayer = VoicemailPlayer()
    private(set) var call: CallPart?
    var inCall: Bool { call?.active.map { $0.phase != .ended } ?? false }
    let media: MediaEngine
    @ObservationIgnored let callWindow = CallWindow()
    @ObservationIgnored let incomingAlert = IncomingCallAlert()
    @ObservationIgnored let ringback = OutgoingRingback()
    @ObservationIgnored private let network = NetworkWatcher()
    /// False while no network path is usable: a banner says so.
    private(set) var online = true
    /// The app's store, for the app delegate (quitting during a call).
    static weak var current: Store?
    let isDemo: Bool
    private(set) var shell: Shell?
    /// Changes on every keystroke: only the editors read it.
    private(set) var composer: ComposerView?
    private(set) var mentionPeople: [PersonView] = []
    /// Separate from `shell` so menu commands are not rebuilt with every shell change.
    private(set) var signedIn = false
    /// My id alone, for views that would otherwise redraw with every shell change (the call stage).
    private(set) var myId = "me"
    private(set) var sidebar: Sidebar?
    /// The open conversation's header, kept apart from the feeds so neither redraws the other.
    private(set) var conversation: ConversationView?
    /// « Alice est en train d’écrire… » under the open chat.
    private(set) var typing: String?
    /// The message image shown full size over the window, if any.
    var zoomedImage: ZoomedImage?
    private(set) var chatFeed = FeedRows()
    private(set) var postFeed = FeedRows()
    private(set) var threadFeed = FeedRows()
    /// Views of the open conversation by id, as the core sends them by difference.
    @ObservationIgnored private var messages = FeedList<MessageView>()
    @ObservationIgnored private var posts = FeedList<PostView>()
    @ObservationIgnored private var replies = FeedList<MessageView>()
    @ObservationIgnored private var feedsChanged = false
    /// Stamps every view received, so feed rows compare by stamp instead of content.
    @ObservationIgnored private var stamp = 0
    /// Settings sub-page shown, or the settings home when nil.
    var settingsPage: SettingsPage?
    private(set) var members: MembersView?
    private(set) var files: FilesView?
    /// OneDrive or the open channel's folder.
    private(set) var browser: BrowserView?
    private(set) var newChat: NewChatView?
    /// Notification settings and the open chat/channel notification sheet.
    private(set) var notifications: NotificationsView?
    /// The user's own status, status message and automatic replies.
    private(set) var me: MeView?
    /// Group chat dialogs and errors of the chat list actions.
    private(set) var chatAdmin: ChatAdminView?
    /// Team creation, gallery and join code dialogs.
    private(set) var teamsAdmin: TeamsAdminView?
    private(set) var gifs: GifsView?
    /// The compose box's message extensions flyout.
    private(set) var extensions: ExtensionsView?
    private(set) var search: SearchView?
    /// One observable box per id: a photo arriving redraws only the avatars showing it.
    @ObservationIgnored private var avatars: [String: ImageBox] = [:]
    @ObservationIgnored private var requestedAvatars: Set<String> = []
    /// One observable box per preview source, filled once decoded off the main thread.
    @ObservationIgnored private var images: [String: ImageBox] = [:]
    /// Previews the core still counts, and those being decoded.
    @ObservationIgnored private var keptImages: Set<String> = []
    @ObservationIgnored private var decoding: Set<String> = []
    @ObservationIgnored private var keptAvatars: Set<String> = []
    @ObservationIgnored private var decodingAvatars: Set<String> = []
    private static let decoder = DispatchQueue(label: "teams.images", qos: .userInitiated)

    /// From the catalog, in the app's language: fetched again by `reloadEmoji`.
    private(set) var composerEmoji = composerEmojis()
    /// Downloaded with the emoticons; empty until the core has it.
    private(set) var stickers = stickerCategories()
    @ObservationIgnored private var core: OxideCore?
    @ObservationIgnored let login = LoginWindow()

    init(demo: Bool) {
        isDemo = demo
        media = demo ? DemoMediaEngine() : WebRTCMediaEngine()
        let listener = Listener()
        listener.store = self
        login.onReturn = { [weak self] url in self?.send(.loginReturned(url: url)) }
        login.onCancel = { [weak self] in self?.send(.loginCancelled) }
        login.onSilentFailure = { [weak self] in self?.send(.silentLoginFailed) }
        LanguagePreference.stored.apply()
        if let caches = FileManager.default.urls(for: .cachesDirectory, in: .userDomainMask).first {
            setCacheDirectory(path: caches.appendingPathComponent(Bundle.main.bundleIdentifier ?? "Oxide").path)
        }
        core = OxideCore(listener: listener, demo: demo)
        TeamsEmojiCatalog.update(composerEmoji)
        Store.current = self
        network.onChange = { [weak self] in self?.send(.call(action: .networkChanged)) }
        network.onStatus = { [weak self] online in
            guard let self, self.online != online else { return }
            self.online = online
            NSAccessibility.post(
                element: NSApp as Any, notification: .announcementRequested,
                userInfo: [
                    .announcement: online ? L("ui.store.back_online") : L("ui.store.offline"),
                    .priority: NSAccessibilityPriorityLevel.high.rawValue,
                ]
            )
            // Back online: fetch what was missed instead of waiting for the next refresh.
            if online && self.signedIn { self.send(.refresh) }
        }
        network.start()
        media.onEvent = { [weak self] event in
            switch event {
            case .localDescription(let callId, let sdp):
                self?.send(.call(action: .localDescription(callId: callId, sdp: sdp)))
            case .renegotiationAnswer(let callId, let sdp):
                self?.send(.call(action: .renegotiationAnswer(callId: callId, sdp: sdp)))
            case .connected(let callId):
                self?.send(.call(action: .mediaConnected(callId: callId)))
            case .failed(let callId, let reason):
                self?.send(.call(action: .mediaFailed(callId: callId, reason: reason)))
            case .screenShareStopped(let callId, let reason):
                self?.send(.call(action: .screenShareStopped(callId: callId, reason: reason)))
            case .cameraStopped(let callId, let reason):
                self?.send(.call(action: .cameraStopped(callId: callId, reason: reason)))
            }
        }
        send(.setRefreshMode(mode: RefreshPreference.stored.core))
        NotificationRouter.shared.store = self
        NotificationRouter.shared.requestPermission()
    }

    /// Rebuilds the core's texts, and the catalogs it named, in the language just applied.
    func setLanguage(_ language: Language) {
        send(.setLanguage(language: language))
        reloadEmoji()
        stickers = stickerCategories()
    }

    /// The catalog is empty until the core has downloaded it.
    private func reloadEmoji() {
        composerEmoji = composerEmojis()
        TeamsEmojiCatalog.update(composerEmoji)
    }

    func send(_ action: Action) {
        core?.send(action: action)
    }
    func setActive(_ active: Bool) {
        core?.setActive(active: active)
    }

    func openSettings(_ page: SettingsPage) {
        settingsPage = page
        send(.navigate(nav: .settings))
    }

    /// Answers a chat message: a quote of it opens the draft, then the composer takes the focus.
    func quote(_ message: MessageView) {
        let words = message.runs.map(\.text).joined()
            .split(whereSeparator: \.isNewline).joined(separator: " ")
        let excerpt = words.count > 160 ? words.prefix(160) + "…" : words
        let author = message.mine ? L("ui.store.you") : message.sender
        let line = L("ui.store.text", ["author": author, "excerpt": excerpt])
        Self.replyEdits(quoting: line).forEach(send)
        focusComposer()
    }
    /// The draft edits opening a reply: the quoted line, then a plain line break, which the core's caret ends after,
    /// where the answer is typed.
    static func replyEdits(quoting line: String) -> [Action] {
        var quote = ComposerText.empty
        quote.block = .quote
        return [
            .replaceDraft(start: 0, length: 0, text: line, mention: nil, emoji: nil, style: quote),
            .replaceDraft(start: UInt32((line as NSString).length), length: 0, text: "\n", mention: nil, emoji: nil,
                          style: ComposerText.empty),
        ]
    }
    func focusComposer() {
        // Kept for a composer that appears afterwards, such as the thread's.
        composerFocusRequested = true
        NotificationCenter.default.post(name: .focusComposer, object: nil)
    }
    @ObservationIgnored private var composerFocusRequested = false
    func takeComposerFocusRequest() -> Bool {
        defer { composerFocusRequested = false }
        return composerFocusRequested
    }

    /// Opens the chat above (-1) or below (1) the selected one in sidebar order, or an end one when none is.
    func selectAdjacentChat(_ offset: Int) {
        guard let chats = sidebar?.chats, !chats.isEmpty else { return }
        // The sidebar shows favorites first.
        let ordered = chats.filter(\.pinned) + chats.filter { !$0.pinned }
        let next: Int
        if let current = ordered.firstIndex(where: \.selected) {
            next = current + offset
        } else {
            next = offset > 0 ? 0 : ordered.count - 1
        }
        guard ordered.indices.contains(next) else { return }
        if shell?.nav != .chat { send(.navigate(nav: .chat)) }
        send(.select(target: Target(team: nil, id: ordered[next].id)))
    }

    /// My latest message that can still be edited in the visible feed, for ↑ in an empty composer.
    nonisolated static func lastEditable(in rows: FeedRows) -> MessageView? {
        for item in rows.items.reversed() {
            switch item {
            case .message(let message, _, _) where message.mine && !message.deleted: return message.editable ? message : nil
            case .post(let post, _) where post.root.mine && !post.root.deleted: return post.root.editable ? post.root : nil
            default: continue
            }
        }
        return nil
    }

    /// Everything the core sent since the previous pass, in order; feed rows are rebuilt once at the end.
    fileprivate func apply(_ events: [CoreEvent]) {
        for event in events {
            switch event {
            case .update(let update): apply(update)
            case .effect(let effect): perform(effect)
            }
        }
        if feedsChanged {
            feedsChanged = false
            rebuildFeeds()
        }
    }

    private func apply(_ update: Update) {
        if let ids = update.avatarIds {
            // Photos the core evicted (all of them after signing out) go and may be asked for again.
            let kept = Set(ids)
            if ids.isEmpty { requestedAvatars.removeAll() }
            var evicted: Set<String> = []
            for (id, box) in avatars where box.image != nil && !kept.contains(id) {
                box.image = nil
                evicted.insert(id)
            }
            if !evicted.isEmpty {
                requestedAvatars = requestedAvatars.filter {
                    !evicted.contains(String($0.prefix(while: { $0 != "|" })))
                }
            }
            loadAvatars(ids)
        }
        if let srcs = update.imageSrcs { loadImages(srcs) }
        if let calendar = update.calendar, self.calendar != calendar { self.calendar = calendar }
        if let call = update.call {
            if self.call != call { self.call = call }
            callWindow.sync(call, store: self)
            incomingAlert.sync(call, store: self)
            ringback.sync(call, store: self)
        }
        if let calls = update.calls {
            if self.calls != calls { self.calls = calls }
            if let id = calls.audioReady, let audio = core?.takeVoicemailAudio(id: id) {
                voicemailPlayer.play(id: id, data: audio.data)
            }
        }
        if let shell = update.shell {
            if self.shell != shell { self.shell = shell }
            if signedIn != shell.signedIn { signedIn = shell.signedIn }
            if myId != shell.me { myId = shell.me }
            let badge = shell.signedIn ? DockBadge.label(chats: shell.unread, activity: shell.activityUnread) : nil
            if NSApp.dockTile.badgeLabel != badge { NSApp.dockTile.badgeLabel = badge }
        }
        if let composer = update.composer, self.composer != composer { self.composer = composer }
        if let people = update.mentionPeople, mentionPeople != people { mentionPeople = people }
        if let sidebar = update.sidebar, self.sidebar != sidebar { self.sidebar = sidebar }
        if let part = update.conversation {
            if conversation != part.view { conversation = part.view }
            // The "more" button and the empty text come from the header.
            feedsChanged = true
        }
        if let feed = update.feed {
            stamp += 1
            messages.apply(feed.messages.ids, feed.messages.changed, stamp: stamp, id: \.id)
            posts.apply(feed.posts.ids, feed.posts.changed, stamp: stamp, id: \.id)
            replies.apply(feed.replies.ids, feed.replies.changed, stamp: stamp, id: \.id)
            feedsChanged = true
        }
        if let part = update.typing, typing != part.text { typing = part.text }
        if let members = update.members, self.members != members { self.members = members }
        if let files = update.files, self.files != files { self.files = files }
        if let browser = update.browser, self.browser != browser { self.browser = browser }
        if let part = update.newChat, newChat != part.dialog { newChat = part.dialog }
        if let notifications = update.notifications, self.notifications != notifications { self.notifications = notifications }
        if let me = update.me, self.me != me { self.me = me }
        if let chatAdmin = update.chatAdmin, self.chatAdmin != chatAdmin { self.chatAdmin = chatAdmin }
        if let teamsAdmin = update.teamsAdmin, self.teamsAdmin != teamsAdmin { self.teamsAdmin = teamsAdmin }
        if let gifs = update.gifs, self.gifs != gifs { self.gifs = gifs }
        if let extensions = update.extensions, self.extensions != extensions { self.extensions = extensions }
        if let search = update.search, self.search != search { self.search = search }
    }

    private func rebuildFeeds() {
        let view = conversation
        var chat: [FeedItem] = []
        var channel: [FeedItem] = []
        var thread: [FeedItem] = []
        if let view, !view.isChannel {
            if view.hasMore {
                chat.append(.more(label: L("ui.store.scroll_for_earlier_messages"), action: .moreFeed, loading: view.loading))
            }
            chat += FeedItem.messages(messages.ordered, bubbles: true, firstUnread: view.firstUnread)
            if let empty = view.emptyText { chat.append(.note(empty)) }
        }
        if let view, view.isChannel {
            if view.hasMore {
                channel.append(.more(label: L("ui.store.scroll_for_earlier_posts"), action: .moreFeed, loading: view.loading))
            }
            channel += posts.ordered.map { FeedItem.post($0.view, stamp: $0.stamp) }
            if let empty = view.emptyText { channel.append(.note(empty)) }
            if let error = view.postsError { channel.append(.postsError(error)) }
        }
        if let thread0 = view?.thread {
            if thread0.hasMore {
                thread.append(.more(label: L("ui.store.scroll_for_earlier_messages"), action: .moreReplies, loading: thread0.loading))
            }
            thread += FeedItem.messages(replies.ordered, bubbles: false)
            if let empty = thread0.emptyText { thread.append(.note(empty)) }
        }
        let key = view.map { "\($0.target.team ?? ""):\($0.target.id)" } ?? ""
        let threadKey = view?.thread.map { "\(key):\($0.post)" } ?? ""
        // Only a list whose rows changed gets a new revision and redraws its table.
        for (rows, key, keyPath) in [
            (chat, key, \Store.chatFeed), (channel, key, \Store.postFeed), (thread, threadKey, \Store.threadFeed),
        ] {
            if self[keyPath: keyPath].items != rows || self[keyPath: keyPath].key != key {
                stamp += 1
                self[keyPath: keyPath] = FeedRows(revision: stamp, key: key, items: rows)
            }
        }
    }

    fileprivate func perform(_ effect: Effect) {
        switch effect {
        case .copy(let value):
            NSPasteboard.copy(value)
        case .notify(let id, let conversationId, let replyChainId, let title, let subtitle, let body,
                     let sound, let urgent, let canReply):
            NotificationRouter.shared.show(
                id: id, conversationId: conversationId, replyChainId: replyChainId, title: title,
                subtitle: subtitle, body: body, sound: sound, urgent: urgent, canReply: canReply
            )
        case .withdrawNotifications(let conversationId):
            NotificationRouter.shared.withdraw(conversationId: conversationId)
        case .openLogin(let url):
            login.open(url)
        case .closeLogin:
            login.close()
        case .silentLogin(let url):
            login.silent(url)
        case .forgetLogin:
            login.forget()
        case .emojiCatalogChanged:
            reloadEmoji()
        case .stickerCatalogChanged:
            stickers = stickerCategories()
        case .quickLook(let path):
            QuickLook.shared.show(path)
        case .downloaded(let path):
            // Bounces the Downloads stack in the Dock, as browsers do.
            DistributedNotificationCenter.default().post(
                name: Notification.Name("com.apple.DownloadFileFinished"), object: path
            )
        case .mediaPrepare(let callId, let iceServers, let video, let muted, let speaker, let cameraLines, let remoteOffer):
            media.prepare(callId: callId, iceServers: iceServers, video: video, muted: muted, speaker: speaker,
                          cameraLines: Int(cameraLines), remoteOffer: remoteOffer)
        case .mediaRemoteDescription(let callId, let sdp):
            media.setRemoteDescription(callId: callId, sdp: sdp, provisional: false)
        case .mediaRemoteProvisionalDescription(let callId, let sdp):
            media.setRemoteDescription(callId: callId, sdp: sdp, provisional: true)
        case .mediaSetMuted(let callId, let muted):
            media.setMuted(callId: callId, muted: muted)
        case .mediaSetCamera(let callId, let on):
            media.setCamera(callId: callId, on: on)
        case .mediaSetSpeaker(let callId, let on):
            media.setSpeaker(callId: callId, on: on)
        case .mediaSetScreenShare(let callId, let on):
            media.setScreenShare(callId: callId, on: on)
        case .mediaSendLimits(let callId, let mid, let sharing, let maxFs, let maxFps, let maxBitrate, let keyframe):
            media.applySendLimits(callId: callId, SendLimits(mid: mid, sharing: sharing, maxFs: Int(maxFs),
                                                             maxFps: maxFps, maxBitrate: Int(maxBitrate), keyframe: keyframe))
        case .mediaClose(let callId):
            media.close(callId: callId)
        case .mediaRemoteOffer(let callId, let sdp):
            media.answerRemoteOffer(callId: callId, sdp: sdp)
        case .mediaOfferRejected(let callId, let retry):
            media.offerRejected(callId: callId, retry: retry)
        case .mediaRestartIce(let callId):
            media.restartIce(callId: callId)
        }
    }

    struct ImageMemory {
        var count = 0
        var bytes = 0
    }
    /// Decoded photos and previews held here, for Settings › Debug.
    func imageMemory() -> (avatars: ImageMemory, previews: ImageMemory) {
        func sum(_ boxes: [String: ImageBox]) -> ImageMemory {
            boxes.values.reduce(into: ImageMemory()) { total, box in
                guard let cg = box.image?.cgImage(forProposedRect: nil, context: nil, hints: nil) else { return }
                total.count += 1
                total.bytes += cg.bytesPerRow * cg.height
            }
        }
        return (sum(avatars), sum(images))
    }

    /// A message image as decoded for the feed, if it is.
    func shownImage(_ src: String) -> NSImage? { images[src]?.image }
    func imageBox(_ src: String) -> ImageBox {
        if let box = images[src] { return box }
        let box = ImageBox()
        images[src] = box
        return box
    }
    func avatarBox(_ id: String) -> ImageBox {
        if let box = avatars[id] { return box }
        let box = ImageBox()
        avatars[id] = box
        return box
    }
    /// Asks the core for a photo once per id and name, however often avatars reappear.
    func requestAvatar(id: String, name: String) {
        guard requestedAvatars.insert("\(id)|\(name)").inserted else { return }
        send(.loadAvatar(id: id, name: name))
    }

    /// Takes newly ready previews' pixels from the core (which keeps only their size) and decodes them off the main thread.
    private func loadImages(_ srcs: [String]) {
        keptImages = Set(srcs)
        for (src, box) in images where box.image != nil && !keptImages.contains(src) {
            box.image = nil
        }
        guard let core else { return }
        for src in srcs where images[src]?.image == nil && decoding.insert(src).inserted {
            Self.decoder.async { [weak self] in
                let made = core.takeImage(src: src).flatMap(Store.image)
                DispatchQueue.main.async {
                    MainActor.assumeIsolated {
                        guard let self else { return }
                        self.decoding.remove(src)
                        if let made, self.keptImages.contains(src) { self.imageBox(src).image = made }
                    }
                }
            }
        }
    }
    /// Same for photos, which the core keeps until they are taken.
    private func loadAvatars(_ ids: [String]) {
        keptAvatars = Set(ids)
        guard let core else { return }
        for id in ids where avatars[id]?.image == nil && decodingAvatars.insert(id).inserted {
            Self.decoder.async { [weak self] in
                let made = core.takeAvatar(id: id).flatMap(Store.image)
                DispatchQueue.main.async {
                    MainActor.assumeIsolated {
                        guard let self else { return }
                        self.decodingAvatars.remove(id)
                        if let made, self.keptAvatars.contains(id) { self.avatarBox(id).image = made }
                    }
                }
            }
        }
    }

    /// Decodes the downloaded file straight at the core's size, so the full-size picture is never held.
    nonisolated private static func image(_ data: ImageData) -> NSImage? {
        let options: [CFString: Any] = [
            kCGImageSourceCreateThumbnailFromImageAlways: true,
            kCGImageSourceCreateThumbnailWithTransform: true,
            kCGImageSourceShouldCacheImmediately: true,
            kCGImageSourceThumbnailMaxPixelSize: max(data.width, data.height),
        ]
        guard let source = CGImageSourceCreateWithData(data.data as CFData, nil),
              let cg = CGImageSourceCreateThumbnailAtIndex(source, 0, options as CFDictionary)
        else { return nil }
        // Pixels are 2× the 420×250 preview box: show them at Retina density.
        return NSImage(cgImage: cg, size: NSSize(width: cg.width / 2, height: cg.height / 2))
    }
}

extension NSPasteboard {
    static func copy(_ text: String) {
        general.clearContents()
        general.setString(text, forType: .string)
    }
}

extension Notification.Name {
    /// Puts the keyboard focus in the visible message composer.
    static let focusComposer = Notification.Name("Oxide.focusComposer")
    /// A row of the feed changed height without changing content; the object is a `FeedRowResize`.
    static let feedRowResized = Notification.Name("Oxide.feedRowResized")
}

/// A row to measure again, and what it shows beyond the copy the feed measures, which has none of its view state.
struct FeedRowResize {
    /// The message's id, which a post's row shows as its root.
    let id: String
    let extra: CGFloat
}

@MainActor
@Observable
final class ImageBox {
    fileprivate(set) var image: NSImage?
}

/// The rows of one feed table; equal revisions mean equal rows, so unrelated redraws return at once.
struct FeedRows: Equatable {
    var revision = 0
    /// The conversation (or thread) the rows belong to.
    var key = ""
    var items: [FeedItem] = []
    static func == (a: FeedRows, b: FeedRows) -> Bool { a.revision == b.revision }
}

/// One list of the open conversation: ids in order, each view with the stamp of the update that brought it.
struct FeedList<View> {
    private(set) var ids: [String] = []
    private var views: [String: (view: View, stamp: Int)] = [:]

    mutating func apply(_ newIds: [String]?, _ changed: [View], stamp: Int, id: (View) -> String) {
        if let newIds {
            ids = newIds
            let kept = Set(newIds)
            views = views.filter { kept.contains($0.key) }
        }
        for view in changed { views[id(view)] = (view, stamp) }
    }
    var ordered: [(view: View, stamp: Int)] { ids.compactMap { views[$0] } }
}

enum CoreEvent {
    case update(Update)
    case effect(Effect)
}

/// Replays core callbacks from Rust threads on the main thread, batching a burst into one pass.
private final class Listener: CoreListener, @unchecked Sendable {
    weak var store: Store?
    private let lock = NSLock()
    private var pending: [CoreEvent] = []

    func update(update: Update) { enqueue(.update(update)) }
    func effect(effect: Effect) { enqueue(.effect(effect)) }

    private func enqueue(_ event: CoreEvent) {
        lock.lock()
        let idle = pending.isEmpty
        pending.append(event)
        lock.unlock()
        // A pass is already due otherwise; it will take this event too.
        guard idle else { return }
        DispatchQueue.main.async { [weak self] in
            MainActor.assumeIsolated { self?.drain() }
        }
    }
    @MainActor private func drain() {
        lock.lock()
        let events = pending
        pending.removeAll()
        lock.unlock()
        store?.apply(events)
    }
}

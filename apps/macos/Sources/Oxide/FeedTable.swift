import AppKit
import SwiftUI
import OxideCore

/// One row of a message feed.
enum FeedItem: Identifiable, Equatable {
    case more(label: String, action: Action, loading: Bool)
    case day(label: String)
    /// `stamp` changes whenever the core sends the message again, so rows compare cheaply.
    case message(MessageView, bubble: Bool, stamp: Int)
    case post(PostView, stamp: Int)
    case note(String)
    case postsError(String)
    /// « Nouveaux messages », before the first message that was unread on opening.
    case unread

    var id: String {
        switch self {
        case .more: "more"
        // Stable while older messages are loaded above it, so its row is kept.
        case .day(let label): "day:\(label)"
        case .unread: "unread"
        case .message(let message, _, _): "message:\(message.id)"
        case .post(let post, _): "post:\(post.id)"
        case .note: "note"
        case .postsError: "posts-error"
        }
    }

    static func == (a: FeedItem, b: FeedItem) -> Bool {
        switch (a, b) {
        case let (.more(l1, a1, d1), .more(l2, a2, d2)): l1 == l2 && a1 == a2 && d1 == d2
        case let (.day(l1), .day(l2)): l1 == l2
        case let (.message(m1, b1, s1), .message(m2, b2, s2)): s1 == s2 && b1 == b2 && m1.id == m2.id
        case let (.post(p1, s1), .post(p2, s2)): s1 == s2 && p1.id == p2.id
        case let (.note(t1), .note(t2)): t1 == t2
        case let (.postsError(e1), .postsError(e2)): e1 == e2
        case (.unread, .unread): true
        default: false
        }
    }

    /// Messages with a separator before each new day, and « Nouveaux messages » before `firstUnread`.
    static func messages(
        _ messages: [(view: MessageView, stamp: Int)], bubbles: Bool, firstUnread: String? = nil
    ) -> [FeedItem] {
        var items: [FeedItem] = []
        var day = ""
        for (message, stamp) in messages {
            if !message.day.isEmpty, message.day != day {
                items.append(.day(label: message.day))
                day = message.day
            }
            if message.id == firstUnread { items.append(.unread) }
            items.append(.message(message, bubble: bubbles, stamp: stamp))
        }
        return items
    }

    /// A message or post written by someone else, for the count of unseen arrivals.
    fileprivate var arrival: Bool? {
        switch self {
        case .message(let message, _, _): message.mine
        case .post(let post, _): post.root.mine
        default: nil
        }
    }
}

/// State of the « ↓ N nouveaux messages » pill shown while the reader is scrolled up, and its jump back.
@MainActor
@Observable
final class FeedController {
    /// Messages from others that arrived below the visible part.
    fileprivate(set) var unseen = 0
    fileprivate(set) var atBottom = true
    @ObservationIgnored fileprivate var jump: (() -> Void)?

    func scrollToBottom() { jump?() }

    fileprivate func update(atBottom: Bool) {
        if self.atBottom != atBottom { self.atBottom = atBottom }
        if atBottom && unseen != 0 { unseen = 0 }
    }
    fileprivate func arrived(_ count: Int) {
        if count > 0 { unseen += count }
    }
}

/// Floats over the bottom of a feed while the reader is scrolled up.
struct NewMessagesPill: View {
    let controller: FeedController
    var body: some View {
        if !controller.atBottom {
            let unseen = controller.unseen
            Button { controller.scrollToBottom() } label: {
                Label(
                    unseen == 0 ? L("ui.feed_table.latest_messages")
                        : unseen == 1 ? L("ui.feed_table.1_new_message")
                        : L("ui.feed_table.new_messages", ["unseen": unseen]),
                    systemImage: "arrow.down"
                )
                .font(.figtree(12, .semibold))
                .contentTransition(.numericText(value: Double(unseen)))
                .foregroundStyle(unseen == 0 ? Theme.text : .white)
                .padding(.horizontal, 14)
                .frame(height: 30)
                .background(unseen == 0 ? Theme.surface : Theme.accent, in: Capsule())
                .overlay(Capsule().strokeBorder(unseen == 0 ? Theme.border : .clear))
                .shadow(color: .black.opacity(0.12), radius: 6, y: 2)
                .contentShape(Capsule())
            }
            .buttonStyle(.plain).pointerCursor()
            .padding(.bottom, 12)
            .help(L("ui.feed_table.jump_to_latest_messages"))
            .animation(Motion.pop, value: unseen)
            .transition(.rise)
        }
    }
}

/// An AppKit table rather than a lazy stack, so rows are measured once per width instead of SwiftUI re-measuring the window.
struct FeedTable: NSViewRepresentable {
    @Environment(Store.self) private var store
    let rows: FeedRows
    var controller: FeedController?

    func makeCoordinator() -> Coordinator { Coordinator() }

    func makeNSView(context: Context) -> NSScrollView {
        let table = FeedTableView()
        table.addTableColumn(NSTableColumn(identifier: .init("feed")))
        table.headerView = nil
        table.style = .plain
        table.selectionHighlightStyle = .none
        table.intercellSpacing = .zero
        table.backgroundColor = .clear
        table.gridStyleMask = []
        table.columnAutoresizingStyle = .uniformColumnAutoresizingStyle
        table.focusRingType = .none
        table.dataSource = context.coordinator
        table.delegate = context.coordinator
        let scroll = NSScrollView()
        scroll.documentView = table
        scroll.drawsBackground = false
        scroll.hasVerticalScroller = true
        scroll.autohidesScrollers = true
        scroll.automaticallyAdjustsContentInsets = false
        context.coordinator.table = table
        context.coordinator.store = store
        context.coordinator.attach(controller)
        context.coordinator.observe(scroll)
        context.coordinator.apply(rows)
        return scroll
    }

    func updateNSView(_ scroll: NSScrollView, context: Context) {
        context.coordinator.store = store
        context.coordinator.attach(controller)
        context.coordinator.apply(rows)
    }

    static func dismantleNSView(_ scroll: NSScrollView, coordinator: Coordinator) {
        coordinator.stop()
    }

    /// Rows keep an estimated height until they near the viewport, so opening or resizing measures a screenful, not the history.
    @MainActor
    final class Coordinator: NSObject, NSTableViewDataSource, NSTableViewDelegate {
        weak var table: FeedTableView?
        var store: Store?
        private var items: [FeedItem] = []
        private var ids: [String] = []
        /// Revision of the rows shown; -1 until the first ones.
        private var revision = -1
        /// Conversation of the rows shown.
        private var key: String?
        private var exact: [String: CGFloat] = [:]
        /// Estimates for `estimatedWidth`, cached because the table asks for every row's height on each reload.
        private var estimates: [String: CGFloat] = [:]
        private var estimatedWidth: CGFloat = 0
        private var measuredWidth: CGFloat = 0
        private var loaded = false
        private var measuring = false
        /// Rows around the viewport left for a later pass once a pass used its time budget.
        private var measureQueued = false
        private var resizeWork: DispatchWorkItem?
        private let sizer = NSHostingController(rootView: FeedRow(store: nil, item: nil))
        private var observers: [NSObjectProtocol] = []
        /// Messages and posts last reported on screen, whose previews the core loads first.
        private var shown: [String] = []
        private var wheelMonitor: Any?
        /// Whether the current scroll gesture goes to the feed rather than a scroll view inside a row.
        private var wheelToFeed = false
        private var earlierGate = EarlierLoadGate()
        private weak var controller: FeedController?

        func attach(_ controller: FeedController?) {
            guard self.controller !== controller else { return }
            self.controller = controller
            controller?.jump = { [weak self] in self?.jumpToBottom() }
        }

        func stop() {
            observers.forEach(NotificationCenter.default.removeObserver)
            observers.removeAll()
            if let wheelMonitor { NSEvent.removeMonitor(wheelMonitor) }
            wheelMonitor = nil
            resizeWork?.cancel()
        }

        func observe(_ scroll: NSScrollView) {
            let clip = scroll.contentView
            clip.postsFrameChangedNotifications = true
            clip.postsBoundsChangedNotifications = true
            observers.append(NotificationCenter.default.addObserver(
                forName: NSView.frameDidChangeNotification, object: clip, queue: .main
            ) { [weak self] _ in
                MainActor.assumeIsolated { self?.widthChanged() }
            })
            observers.append(NotificationCenter.default.addObserver(
                forName: NSView.boundsDidChangeNotification, object: clip, queue: .main
            ) { [weak self] _ in
                MainActor.assumeIsolated {
                    self?.measureVisible()
                    self?.reportPosition()
                }
            })
            wheelMonitor = NSEvent.addLocalMonitorForEvents(matching: .scrollWheel) { [weak self, weak scroll] event in
                let routed = MainActor.assumeIsolated {
                    guard let self, let scroll else { return false }
                    self.noteGesture(event, on: scroll)
                    return self.routeWheel(event, to: scroll)
                }
                return routed ? nil : event
            }
            guard let table else { return }
            table.press = { [weak self] event, text in
                self?.press(event, on: text) ?? false
            }
            table.selection.rows = { [weak self] in
                guard let self else { return [:] }
                return Dictionary(self.items.enumerated().compactMap { row, item in
                    Self.message(of: item).map { ($0.message.id, row) }
                }, uniquingKeysWith: { first, _ in first })
            }
            table.selection.units = { [weak self] row in
                guard let self, row < self.items.count, let (message, bubble) = Self.message(of: self.items[row]) else {
                    return []
                }
                let size = MessageBody.emojiOnly(message.blocks) ? 30.0 : 14.0
                let author = (FeedSelection.Unit(message: message.id, segment: FeedSelection.Unit.authorLine),
                              MessageRow.authorLine(message, bubble: bubble))
                return [author] + MessageRichText.pieces(message.blocks, size: size).enumerated().compactMap { index, piece in
                    piece.selectable.map { (FeedSelection.Unit(message: message.id, segment: index), $0) }
                }
            }
        }

        private static func message(of item: FeedItem) -> (message: MessageView, bubble: Bool)? {
            switch item {
            case .message(let message, let bubble, _): (message, bubble)
            case .post(let post, _): (post.root, false)
            default: nil
            }
        }

        /// A plain press on a message's text or anywhere around it, as above it or beside its author, starts a selection
        /// at the nearest text, as in Teams. Returns whether it took the press; double clicks and links are left alone.
        private func press(_ event: NSEvent, on text: MessageTextView?) -> Bool {
            guard let table else { return false }
            table.selection.clear()
            guard event.clickCount == 1, event.modifierFlags.intersection([.command, .control, .shift]).isEmpty else {
                return false
            }
            if let text, Self.onLink(event, in: text) { return false }
            let target: (MessageTextView, FeedSelection.Point)?
            if let text, let unit = text.unit {
                target = (text, FeedSelection.Point(unit: unit, index: Self.index(of: event, in: text)))
            } else {
                target = Self.point(of: event, in: table)
            }
            guard let (text, start) = target else { return false }
            trackSelection(from: event, in: text, at: start, table: table)
            return true
        }

        /// Follows a drag across the feed, scrolling it at its edges, as a web page does.
        private func trackSelection(from event: NSEvent, in text: MessageTextView, at start: FeedSelection.Point,
                                    table: FeedTableView) {
            guard let window = table.window else { return }
            window.makeFirstResponder(text)
            text.setSelectedRange(NSRange(location: start.index, length: 0))
            table.selection.begin(at: start)
            NSEvent.startPeriodicEvents(afterDelay: 0.1, withPeriod: 0.05)
            defer { NSEvent.stopPeriodicEvents() }
            var last = event
            while let next = window.nextEvent(matching: [.leftMouseDragged, .leftMouseUp, .periodic]) {
                if next.type == .leftMouseUp { break }
                if next.type == .leftMouseDragged { last = next }
                table.autoscroll(with: last)
                if let (_, point) = Self.point(of: last, in: table) { table.selection.extend(to: point) }
            }
        }

        /// The text position under a drag: the nearest message text of the row under it, clamped to its ends.
        private static func point(of event: NSEvent, in table: NSTableView) -> (MessageTextView, FeedSelection.Point)? {
            let location = table.convert(event.locationInWindow, from: nil)
            let visible = table.rows(in: table.visibleRect)
            guard visible.location != NSNotFound, visible.length > 0 else { return nil }
            var row = table.row(at: location)
            if row < 0 {
                row = location.y < table.visibleRect.midY ? visible.location : visible.location + visible.length - 1
            }
            guard let rowView = table.rowView(atRow: row, makeIfNecessary: false) else { return nil }
            let texts = rowView.descendants(of: MessageTextView.self).filter { $0.unit != nil }
            // Texts are stacked in order; the drag takes the last one starting above it, or the first.
            guard let text = texts.last(where: { $0.convert($0.bounds, to: nil).maxY >= event.locationInWindow.y })
                    ?? texts.first, let unit = text.unit else { return nil }
            return (text, FeedSelection.Point(unit: unit, index: index(of: event, in: text)))
        }

        /// The character position under `event` in `text`; above its first line it is the start, below its last the end.
        private static func index(of event: NSEvent, in text: MessageTextView) -> Int {
            let point = text.convert(event.locationInWindow, from: nil)
            let length = text.textStorage?.length ?? 0
            if point.y < text.bounds.minY { return 0 }
            if point.y > text.bounds.maxY { return length }
            let clamped = NSPoint(x: min(max(point.x, text.bounds.minX), text.bounds.maxX), y: point.y)
            return min(text.characterIndexForInsertion(at: clamped), length)
        }

        /// A press on a link opens it, as the text view does on release.
        private static func onLink(_ event: NSEvent, in text: MessageTextView) -> Bool {
            guard let storage = text.textStorage, let layout = text.layoutManager, let container = text.textContainer,
                  storage.length > 0 else { return false }
            let point = text.convert(event.locationInWindow, from: nil)
            let glyph = layout.glyphIndex(for: point, in: container)
            guard layout.boundingRect(forGlyphRange: NSRange(location: glyph, length: 1), in: container).contains(point) else {
                return false
            }
            let character = layout.characterIndexForGlyph(at: glyph)
            return character < storage.length && storage.attribute(.link, at: character, effectiveRange: nil) != nil
        }

        /// Loads earlier history on a deliberate scroll past the top, as Teams does: see `EarlierLoadGate`.
        private func noteGesture(_ event: NSEvent, on scroll: NSScrollView) {
            guard event.window === scroll.window,
                  scroll.bounds.contains(scroll.convert(event.locationInWindow, from: nil)),
                  let table else { return }
            let action = FeedItem.earlierAction(items, visible: table.rows(in: scroll.contentView.bounds))
            if event.phase.contains(.began) {
                earlierGate.gestureBegan(rowOnScreen: action != nil)
            } else if event.phase.isEmpty && event.momentumPhase.isEmpty {
                earlierGate.wheelTick(at: event.timestamp, rowOnScreen: action != nil)
            }
            if event.momentumPhase.isEmpty, event.scrollingDeltaY > 0, let action = earlierGate.pass(action) {
                store?.send(action)
            }
        }

        /// A sideways scroll view inside a row (a code block) would otherwise swallow vertical scrolling while under the pointer;
        /// one that scrolls vertically and has more content than height keeps it. Each gesture is routed as a whole, from the direction it starts in.
        private func routeWheel(_ event: NSEvent, to scroll: NSScrollView) -> Bool {
            guard event.window === scroll.window else { return false }
            if event.phase.contains(.began) || (event.phase.isEmpty && event.momentumPhase.isEmpty) {
                let hit = scroll.window?.contentView?.hitTest(event.locationInWindow)
                let inner = hit?.enclosingScrollView.flatMap { $0 !== scroll && $0.isDescendant(of: scroll) ? $0 : nil }
                let innerScrollsDown = inner.map { $0.hasVerticalScroller && ($0.documentView?.frame.height ?? 0) > $0.contentView.bounds.height } ?? false
                wheelToFeed = inner != nil && !innerScrollsDown && abs(event.scrollingDeltaY) > abs(event.scrollingDeltaX)
            }
            guard wheelToFeed else { return false }
            scroll.scrollWheel(with: event)
            return true
        }

        func apply(_ rows: FeedRows) {
            // SwiftUI calls this whenever the screen around the table redraws.
            guard let table, rows.revision != revision else { return }
            revision = rows.revision
            // Another conversation opens at its bottom, whatever the previous one showed.
            if rows.key != key {
                key = rows.key
                loaded = false
                // Heights of the previous conversation's rows would otherwise pile up.
                let keep = Set(rows.items.map(\.id))
                exact = exact.filter { keep.contains($0.key) }
                estimates = estimates.filter { keep.contains($0.key) }
            }
            let new = rows.items
            let old = items
            let oldIds = ids
            let newIds = new.map(\.id)
            var follow = !loaded || isAtBottom()
            if oldIds == newIds {
                let changed = IndexSet(new.indices.filter { old[$0] != new[$0] })
                guard !changed.isEmpty else { return }
                items = new
                for index in changed { forget(new[index].id) }
                table.reloadData(forRowIndexes: changed, columnIndexes: [0])
                withoutAnimation { table.noteHeightOfRows(withIndexesChanged: changed) }
            } else {
                let anchor = firstVisible()
                // Only rows added or removed move; the other cells stay as they are.
                var removed = IndexSet()
                var inserted = IndexSet()
                for change in newIds.difference(from: oldIds) {
                    switch change {
                    case .remove(let offset, _, _): removed.insert(offset)
                    case .insert(let offset, _, _): inserted.insert(offset)
                    }
                }
                // Rows below the last kept one are arrivals: mine follow to the bottom, others' are counted.
                let kept = Set(oldIds)
                var arrivals = 0
                if let last = newIds.lastIndex(where: kept.contains) {
                    for index in inserted where index > last {
                        switch new[index].arrival {
                        case true?: follow = true
                        case false?: arrivals += 1
                        case nil: break
                        }
                    }
                } else {
                    follow = true
                }
                if !follow { controller?.arrived(arrivals) }
                let previous = Dictionary(zip(oldIds, old), uniquingKeysWith: { a, _ in a })
                for item in new where previous[item.id] != item { forget(item.id) }
                items = new
                ids = newIds
                let changed = IndexSet(new.indices.filter {
                    !inserted.contains($0) && previous[new[$0].id] != new[$0]
                })
                withoutAnimation {
                    table.beginUpdates()
                    table.removeRows(at: removed, withAnimation: [])
                    table.insertRows(at: inserted, withAnimation: [])
                    table.endUpdates()
                    if !changed.isEmpty {
                        table.reloadData(forRowIndexes: changed, columnIndexes: [0])
                        table.noteHeightOfRows(withIndexesChanged: changed)
                    }
                }
                if !follow, let anchor { restore(anchor) }
            }
            if !new.isEmpty { loaded = true }
            updateInset()
            if follow { scrollToBottom() }
            measureVisible()
            reportPosition()
        }

        private func reportPosition() {
            controller?.update(atBottom: isAtBottom())
        }
        /// The pill's action: measured rows may move the bottom, so it is reached again.
        private func jumpToBottom() {
            scrollToBottom()
            measureVisible()
            scrollToBottom()
            reportPosition()
        }

        // MARK: Table

        func numberOfRows(in tableView: NSTableView) -> Int { items.count }

        func tableView(_ tableView: NSTableView, heightOfRow row: Int) -> CGFloat {
            let item = items[row]
            if let height = exact[item.id] { return height }
            let width = tableView.bounds.width
            if abs(width - estimatedWidth) > 0.5 {
                estimates.removeAll()
                estimatedWidth = width
            }
            if let height = estimates[item.id] { return height }
            let height = estimate(item, width: width)
            estimates[item.id] = height
            return height
        }

        func tableView(_ tableView: NSTableView, viewFor tableColumn: NSTableColumn?, row: Int) -> NSView? {
            let identifier = NSUserInterfaceItemIdentifier("feed-row")
            let cell = tableView.makeView(withIdentifier: identifier, owner: nil) as? HostCell ?? HostCell()
            cell.identifier = identifier
            cell.host.rootView = FeedRow(store: store, item: items[row])
            return cell
        }

        func tableView(_ tableView: NSTableView, shouldSelectRow row: Int) -> Bool { false }

        private func forget(_ id: String) {
            exact[id] = nil
            estimates[id] = nil
        }

        // MARK: Heights

        /// Measures the rows on screen and one screen around it, keeping the reading position.
        /// Rows on screen are always measured; the others stop after 4 ms and continue on the next run loop pass.
        private func measureVisible() {
            guard !measuring, let table, let clip = table.enclosingScrollView?.contentView, !items.isEmpty else { return }
            measuring = true
            defer { measuring = false }
            let width = max(table.bounds.width, 1)
            measuredWidth = width
            let around = clip.bounds.insetBy(dx: 0, dy: -clip.bounds.height)
            let rows = table.rows(in: around)
            guard rows.location != NSNotFound else { return }
            let all = rows.location..<min(rows.location + rows.length, items.count)
            reportShown(all)
            let shown = table.rows(in: clip.bounds)
            let visible = shown.location == NSNotFound
                ? all.lowerBound..<all.lowerBound
                : all.clamped(to: shown.location..<(shown.location + shown.length))
            let order = Array(visible) + all.filter { !visible.contains($0) }
            let deadline = CACurrentMediaTime() + 0.004
            var changed = IndexSet()
            var unfinished = false
            for row in order {
                let item = items[row]
                guard exact[item.id] == nil else { continue }
                if !visible.contains(row) && CACurrentMediaTime() > deadline {
                    unfinished = true
                    break
                }
                sizer.rootView = FeedRow(store: store, item: item)
                let size = sizer.sizeThatFits(in: CGSize(width: width, height: .greatestFiniteMagnitude))
                exact[item.id] = max(ceil(size.height), 1)
                changed.insert(row)
            }
            if unfinished && !measureQueued {
                measureQueued = true
                DispatchQueue.main.async { [weak self] in
                    MainActor.assumeIsolated {
                        self?.measureQueued = false
                        self?.measureVisible()
                    }
                }
            }
            guard !changed.isEmpty else { return }
            keepingPosition {
                withoutAnimation { table.noteHeightOfRows(withIndexesChanged: changed) }
                updateInset()
            }
        }

        private func reportShown(_ rows: Range<Int>) {
            let ids = rows.compactMap { row -> String? in
                switch items[row] {
                case .message(let message, _, _): message.id
                case .post(let post, _): post.id
                default: nil
                }
            }
            guard ids != shown else { return }
            shown = ids
            store?.send(.showMessages(ids: ids))
        }

        /// Rough height from the content, replaced by the real one before the row is seen.
        private func estimate(_ item: FeedItem, width: CGFloat) -> CGFloat {
            func message(_ m: MessageView) -> CGFloat {
                let characters = m.runs.reduce(0) { $0 + $1.text.count }
                let perLine = max(Int((min(width, 640) - 120) / 7.5), 20)
                let lines = max(1, characters / perLine + m.runs.filter { $0.text.contains("\n") }.count)
                // Block chrome the flat text does not show: quoted replies, code headers, cards.
                let blocks = m.blocks.reduce(CGFloat(0)) { total, block in
                    switch block {
                    case .quote: total + 70
                    case .code: total + 48
                    case .table(_, let rows): total + CGFloat(rows.count) * 12
                    case .card(let card): total + 60 + CGFloat(card.items.count) * 24
                    default: total + 8
                    }
                }
                let images = m.images.reduce(CGFloat(0)) { $0 + PreviewImage.estimatedHeight($1, width: min(width, 640) - 120) + 8 }
                let link: CGFloat = m.linkPreview.map { $0.image == nil ? 90 : 240 } ?? 0
                return 56 + CGFloat(lines) * 21 + blocks + images + CGFloat(m.files.count) * 66
                    + CGFloat(m.files.filter { if case .ready = $0.thumbnail?.status { true } else { false } }.count) * 158
                    + link
                    + (m.reactions.isEmpty ? 0 : 30) + (m.subject == nil ? 0 : 26)
                    + (m.importance == .standard ? 0 : 22)
            }
            switch item {
            case .more: return 36
            case .day, .unread: return 32
            case .note: return 64
            case .postsError: return 90
            case .message(let m, let bubble, _): return message(m) + (bubble ? 20 : 17)
            case .post(let post, _) where post.root.deleted: return 40
            case .post(let post, _): return message(post.root) + 104
            }
        }

        private func widthChanged() {
            updateInset()
            guard let table, abs(table.bounds.width - measuredWidth) > 0.5, !items.isEmpty else { return }
            // A live resize changes the width every frame: re-measure once the drag pauses.
            if table.inLiveResize {
                resizeWork?.cancel()
                let work = DispatchWorkItem { [weak self] in
                    MainActor.assumeIsolated { self?.remeasure() }
                }
                resizeWork = work
                DispatchQueue.main.asyncAfter(deadline: .now() + 0.08, execute: work)
                return
            }
            remeasure()
        }
        private func remeasure() {
            guard let table, abs(table.bounds.width - measuredWidth) > 0.5, !items.isEmpty else { return }
            keepingPosition {
                exact.removeAll()
                withoutAnimation { table.noteHeightOfRows(withIndexesChanged: IndexSet(integersIn: 0..<items.count)) }
            }
            measureVisible()
        }

        // MARK: Scrolling

        /// Height of the rows themselves: the table view also stretches to fill the clip view.
        private var contentHeight: CGFloat {
            guard let table, !items.isEmpty else { return 0 }
            return table.rect(ofRow: items.count - 1).maxY
        }
        /// A feed shorter than the view sits at the bottom, like a chat.
        private func updateInset() {
            guard let table, let scroll = table.enclosingScrollView else { return }
            let top = max(scroll.frame.height - contentHeight, 0)
            if abs(scroll.contentInsets.top - top) > 0.5 {
                scroll.contentInsets = NSEdgeInsets(top: top, left: 0, bottom: 0, right: 0)
            }
        }
        private func isAtBottom() -> Bool {
            guard let table, let clip = table.enclosingScrollView?.contentView else { return true }
            return clip.bounds.maxY >= contentHeight - 40
        }
        private func firstVisible() -> (id: String, offset: CGFloat)? {
            guard let table, let clip = table.enclosingScrollView?.contentView else { return nil }
            let rows = table.rows(in: clip.bounds)
            guard rows.location != NSNotFound, rows.location < items.count else { return nil }
            return (items[rows.location].id, table.rect(ofRow: rows.location).minY - clip.bounds.minY)
        }
        private func restore(_ anchor: (id: String, offset: CGFloat)) {
            guard let table, let index = items.firstIndex(where: { $0.id == anchor.id }) else { return }
            scroll(to: table.rect(ofRow: index).minY - anchor.offset)
        }
        /// Stays at the bottom if it was there, else keeps the first visible row in place.
        private func keepingPosition(_ change: () -> Void) {
            let follow = isAtBottom()
            let anchor = firstVisible()
            change()
            if follow {
                scrollToBottom()
            } else if let anchor {
                restore(anchor)
            }
        }
        private func scrollToBottom() {
            let clip = table?.enclosingScrollView?.contentView
            scroll(to: contentHeight - (clip?.bounds.height ?? 0))
        }
        private func scroll(to y: CGFloat) {
            guard let table, let scroll = table.enclosingScrollView else { return }
            table.layoutSubtreeIfNeeded()
            let minY = -scroll.contentInsets.top
            let maxY = max(contentHeight - scroll.contentView.bounds.height, minY)
            scroll.contentView.scroll(to: NSPoint(x: 0, y: min(max(y, minY), maxY)))
            scroll.reflectScrolledClipView(scroll.contentView)
        }
        private func withoutAnimation(_ change: () -> Void) {
            NSAnimationContext.beginGrouping()
            NSAnimationContext.current.duration = 0
            change()
            NSAnimationContext.endGrouping()
        }
    }
}

/// Lets "load earlier" through only for a scroll gesture that starts with its row on screen: reaching the row,
/// momentum included, only shows it, and going on loads one page per gesture.
struct EarlierLoadGate {
    /// A mouse wheel has no gestures: a pause this long between its ticks ends one, so a free-spinning wheel counts once.
    static let wheelPause: TimeInterval = 0.3
    private var armed = false
    private var lastTick = -TimeInterval.infinity
    mutating func gestureBegan(rowOnScreen: Bool) { armed = rowOnScreen }
    mutating func wheelTick(at time: TimeInterval, rowOnScreen: Bool) {
        if time - lastTick >= Self.wheelPause { armed = rowOnScreen }
        lastTick = time
    }
    /// `action` if the gesture may still load a page, which it then may not until the next gesture.
    mutating func pass(_ action: Action?) -> Action? {
        guard armed, let action else { return nil }
        armed = false
        return action
    }
}

extension FeedItem {
    /// The action of the idle "load earlier" row heading `items`, when `visible` rows include it.
    static func earlierAction(_ items: [FeedItem], visible: NSRange) -> Action? {
        guard case .more(_, let action, false) = items.first,
              visible.location == 0, visible.length > 0 else { return nil }
        return action
    }
}

/// A row with its store, as one concrete type: cells swap rows without type erasure.
struct FeedRow: View {
    let store: Store?
    let item: FeedItem?
    var body: some View {
        if let store, let item {
            FeedItemView(item: item).environment(store)
        }
    }
}

private final class HostCell: NSTableCellView {
    let host = NSHostingView(rootView: FeedRow(store: nil, item: nil))
    init() {
        super.init(frame: .zero)
        // The table sets the row height; SwiftUI need not size the cell on every reuse.
        host.sizingOptions = []
        host.translatesAutoresizingMaskIntoConstraints = false
        addSubview(host)
        NSLayoutConstraint.activate([
            host.leadingAnchor.constraint(equalTo: leadingAnchor),
            host.trailingAnchor.constraint(equalTo: trailingAnchor),
            host.topAnchor.constraint(equalTo: topAnchor),
            host.bottomAnchor.constraint(equalTo: bottomAnchor),
        ])
    }
    required init?(coder: NSCoder) { fatalError("init(coder:) is not used") }
}

/// Renders one feed row; vertical padding replaces the stack spacing.
struct FeedItemView: View {
    @Environment(Store.self) private var store
    let item: FeedItem
    var body: some View {
        content
            .padding(.horizontal, 24)
            .padding(.vertical, 8)
            .frame(maxWidth: .infinity, alignment: .leading)
            .font(.figtree(13))
            .foregroundStyle(Theme.text)
    }
    @ViewBuilder private var content: some View {
        switch item {
        case .more(let label, let action, let loading):
            // Scrolling up to the row loads it, as in Teams; the same height either way keeps the feed still.
            ZStack {
                if loading {
                    ProgressView().controlSize(.small)
                } else {
                    Text(label).font(.figtree(12)).foregroundStyle(Theme.subtle)
                }
            }
            .frame(maxWidth: .infinity, minHeight: 20)
            .accessibilityElement()
            .accessibilityLabel(label)
            .accessibilityAddTraits(.isButton)
            .accessibilityAction { store.send(action) }
        case .day(let label):
            HStack(spacing: 12) {
                Rectangle().fill(Theme.border).frame(height: 1)
                Text(label).font(.figtree(12)).foregroundStyle(Theme.subtle).fixedSize()
                Rectangle().fill(Theme.border).frame(height: 1)
            }
        case .unread:
            HStack(spacing: 12) {
                Rectangle().fill(Theme.danger).frame(height: 1)
                Text(L("ui.feed_table.new_messages_2")).font(.figtree(12, .semibold)).foregroundStyle(Theme.danger).fixedSize()
                Rectangle().fill(Theme.danger).frame(height: 1)
            }
            .accessibilityElement(children: .combine)
        case .message(let message, let bubble, _):
            if bubble {
                MessageRow(message: message, bubble: true) { store.quote(message) }
            } else {
                // Thread replies: a rule under each one marks where the next speaker starts.
                VStack(alignment: .leading, spacing: 12) {
                    MessageRow(message: message, bubble: false) { store.quote(message) }
                    Rectangle().fill(Theme.border).frame(height: 1).padding(.leading, 42)
                }
                .padding(.top, 4)
            }
        case .post(let post, _) where post.root.deleted:
            // A deleted post keeps one line, and the way to its replies if it has some.
            HStack(spacing: 8) {
                Image(systemName: "trash").font(.system(size: 11)).foregroundStyle(Theme.subtle)
                Text(L("ui.message_row.this_message_has_been_deleted"))
                    .font(.figtree(12)).italic().foregroundStyle(Theme.muted)
                Text("· \(post.root.sender) · \(post.root.time)")
                    .font(.figtree(11)).foregroundStyle(Theme.subtle).lineLimit(1)
                Spacer(minLength: 8)
                if post.replies > 0 {
                    Button(L("ui.feed_table.replies_open_thread", ["replies": post.replies])) {
                        store.send(.openReply(post: post.id))
                    }
                    .buttonStyle(GhostButtonStyle(accent: true))
                    .font(.figtree(12))
                }
            }
            .padding(.horizontal, 14).padding(.vertical, 8)
            .frame(maxWidth: .infinity, alignment: .leading)
            .background(Theme.surface.opacity(0.6), in: RoundedRectangle(cornerRadius: 10))
            .overlay(RoundedRectangle(cornerRadius: 10).strokeBorder(Theme.border))
            .accessibilityElement(children: .combine)
        case .post(let post, _):
            VStack(alignment: .leading, spacing: 14) {
                MessageRow(message: post.root, bubble: false) {
                    store.send(.openReply(post: post.id))
                    store.focusComposer()
                }
                Rectangle().fill(Theme.border).frame(height: 1)
                HStack {
                    Button {
                        store.send(.openReply(post: post.id))
                    } label: {
                        Label(
                            post.replies == 0 ? L("ui.feed_table.reply")
                                : L("ui.feed_table.replies_open_thread", ["replies": post.replies]),
                            systemImage: "bubble.left"
                        )
                        .font(.figtree(12))
                        .padding(.vertical, 4)
                    }
                    .buttonStyle(GhostButtonStyle(accent: true))
                    Spacer()
                    Button {
                        store.send(.notifications(action: .setPostFollow(post: post.id, follow: !post.following)))
                    } label: {
                        Label(post.following ? L("ui.feed_table.following") : L("ui.feed_table.follow"), systemImage: post.following ? "bell.fill" : "bell")
                            .font(.figtree(12))
                            .padding(.vertical, 4)
                    }
                    .buttonStyle(GhostButtonStyle())
                    .help(post.following ? L("ui.feed_table.stop_getting_notified_of_replies") : L("ui.feed_table.get_notified_of_replies"))
                }
            }
            .frame(maxWidth: .infinity, alignment: .leading)
            .card(padding: 18)
        case .note(let text):
            Muted(text, size: 13).padding(16).frame(maxWidth: .infinity)
        case .postsError(let error):
            VStack(alignment: .leading, spacing: 8) {
                Muted(error)
                Button(L("ui.feed_table.retry_posts")) { store.send(.refresh) }
                    .buttonStyle(OutlineButtonStyle())
            }
        }
    }
}

/// The feed's table, which lets the coordinator take presses on its rows' empty areas and texts to select across messages.
final class FeedTableView: NSTableView {
    let selection = FeedSelection()
    var press: ((NSEvent, MessageTextView?) -> Bool)?

    override func mouseDown(with event: NSEvent) {
        if press?(event, nil) == true { return }
        super.mouseDown(with: event)
    }
}

private extension NSView {
    func descendants<View: NSView>(of type: View.Type) -> [View] {
        subviews.flatMap { ($0 as? View).map { [$0] } ?? $0.descendants(of: type) }
    }
}

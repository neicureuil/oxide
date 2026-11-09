import AppKit
import SwiftUI
import OxideCore

extension ComposerView {
    /// Before the first core update, which also brings the shell.
    static var empty: ComposerView {
        ComposerView(
            sending: false, draft: "", draftRev: 0, draftCursor: 0,
            mentionRanges: [], draftFormats: [], mentionLoading: false, mentionError: nil,
            subject: "", importance: .standard, attachments: [], cards: [], linkPreview: nil
        )
    }
}

/// Changes to the core's rich draft from the editor and the tools around it.
@MainActor
struct RichDraft {
    let store: Store
    let input: DraftInput

    var composer: ComposerView { store.composer ?? .empty }
    /// The people the mention being typed can still name.
    var people: [PersonView] {
        guard let mention = input.mention else { return [] }
        return Array(store.mentionPeople.filter {
            mention.query.isEmpty || ($0.name + " " + $0.email).localizedStandardContains(mention.query)
        }.prefix(8))
    }
    func replace(_ text: String, range: NSRange? = nil, mri: String? = nil, emojiID: String? = nil) {
        let selection = range ?? input.selection
        store.send(.replaceDraft(start: UInt32(selection.location), length: UInt32(selection.length), text: text, mention: mri, emoji: emojiID, style: input.currentFormat))
    }
    func choose(_ person: PersonView) {
        guard let mention = input.mention else { return }
        input.dismiss(afterInsertion: true)
        replace("@\(person.name) ", range: mention.range, mri: person.mri)
    }
    /// The editor's keys while a mention is typed: they move in and pick from the list.
    func mentionCommand(_ selector: Selector) -> Bool {
        guard input.mention != nil else { return false }
        let people = self.people
        if selector == #selector(NSResponder.cancelOperation(_:)) { input.dismiss(); return true }
        if selector == #selector(NSResponder.moveDown(_:)) {
            input.highlighted = min(input.highlighted + 1, max(people.count - 1, 0)); return true
        }
        if selector == #selector(NSResponder.moveUp(_:)) {
            input.highlighted = max(input.highlighted - 1, 0); return true
        }
        if selector == #selector(NSResponder.insertNewline(_:)) || selector == #selector(NSResponder.insertTab(_:)) {
            if !people.isEmpty { choose(people[min(input.highlighted, people.count - 1)]) }
            return true
        }
        return false
    }
}

/// The editor bound to the core's draft, with what it shares between the composer and a message being edited:
/// mentions looked up as they are typed, pasted images, and the formatting tools opened for a link.
struct RichDraftEditor: View {
    @Environment(Store.self) private var store
    let placeholder: String
    let input: DraftInput
    @Binding var formatting: Bool
    @Binding var height: CGFloat
    let onSend: () -> Void
    /// Keys the editor did not use for a mention.
    var onCommand: (Selector) -> Bool = { _ in false }
    var onEditLast: () -> Bool = { false }
    var onDropFiles: ([String]) -> Void = { _ in }
    let takeFocusRequest: () -> Bool

    var body: some View {
        let draft = RichDraft(store: store, input: input)
        let composer = draft.composer
        DraftEditor(
            placeholder: placeholder,
            draft: composer.draft,
            revision: composer.draftRev,
            cursor: Int(composer.draftCursor),
            mentions: composer.mentionRanges,
            formats: composer.draftFormats,
            expanded: formatting,
            input: input,
            height: $height,
            onChange: { store.send(.richDraftChanged(value: $0, formats: $1)) },
            onSend: onSend,
            onPasteImage: pasteImage,
            onMentionCommand: { draft.mentionCommand($0) || onCommand($0) },
            onEditLast: onEditLast,
            onDropFiles: onDropFiles,
            takeFocusRequest: takeFocusRequest
        )
        .onChange(of: input.linkRequested) { _, requested in if requested { formatting = true } }
        .onChange(of: input.mention) { _, mention in
            input.highlighted = 0
            if mention != nil { store.send(.mentionMembers) }
        }
    }
    /// Sends a pasted image with its size in points, so a Retina screenshot shows at the size it had on screen.
    private func pasteImage(id: String, png: Data, size: NSSize) {
        store.send(.files(action: .pasteImage(id: id, png: png, width: UInt32(max(size.width.rounded(), 1)),
                                              height: UInt32(max(size.height.rounded(), 1)))))
    }
}

/// The people a mention being typed can name.
struct MentionPicker: View {
    let draft: RichDraft

    var body: some View {
        // Filtered once per drawing rather than once per row.
        let people = draft.people
        let composer = draft.composer
        let input = draft.input
        VStack(alignment: .leading, spacing: 2) {
            Text(L("ui.composer.mention_someone")).font(.figtree(11, .semibold)).foregroundStyle(Theme.muted).padding(6)
            ForEach(Array(people.enumerated()), id: \.element.mri) { index, person in
                Button { draft.choose(person) } label: {
                    HStack(spacing: 10) {
                        Avatar(name: person.name, id: person.mri, size: 28)
                        VStack(alignment: .leading, spacing: 2) {
                            Text(person.name).font(.figtree(13, .semibold))
                            if !person.email.isEmpty { Text(person.email).font(.figtree(11)).foregroundStyle(Theme.muted) }
                        }
                        Spacer()
                    }
                    .padding(6).frame(maxWidth: .infinity, alignment: .leading)
                    .background(index == min(input.highlighted, max(people.count - 1, 0)) ? Theme.accentSoft : .clear, in: RoundedRectangle(cornerRadius: 6))
                    .contentShape(Rectangle())
                }
                .buttonStyle(.plain).pointerCursor().accessibilityLabel(L("ui.composer.mention", ["name": person.name]))
            }
            if composer.mentionLoading {
                HStack { ProgressView().controlSize(.small); Text(L("ui.composer.loading_participants")).font(.figtree(12)) }.padding(6)
            } else if let error = composer.mentionError {
                HStack {
                    Text(L("ui.composer.couldn_t_load_all_participants")).font(.figtree(12))
                    Button(L("ui.composer.try_again")) { draft.store.send(.channelMembers) }
                }.padding(6).help(error)
            } else if people.isEmpty {
                Text(L("ui.composer.no_matching_participants")).font(.figtree(12)).foregroundStyle(Theme.muted).padding(6)
            }
        }
        .padding(4).background(Theme.surface, in: RoundedRectangle(cornerRadius: 12))
        .overlay(RoundedRectangle(cornerRadius: 12).strokeBorder(Theme.borderStrong))
    }
}

/// Shows or hides the formatting toolbar.
struct FormattingToggle: View {
    @Binding var formatting: Bool

    var body: some View {
        Button { formatting.toggle() } label: {
            Text("Aa").font(.figtree(13, .semibold)).frame(width: 32, height: 32)
        }
        .buttonStyle(GhostButtonStyle(accent: formatting))
        .background(formatting ? Theme.accentSoft : .clear, in: RoundedRectangle(cornerRadius: 6))
        .help(formatting ? L("ui.composer.hide_formatting_options") : L("ui.composer.show_formatting_options"))
        .accessibilityLabel(L("ui.composer.format"))
    }
}

/// Inserts an emoji at the caret.
struct EmojiButton: View {
    let draft: RichDraft
    @State private var open = false

    var body: some View {
        IconButton(symbol: "face.smiling", help: L("ui.composer.add_an_emoji"), side: 32) { open.toggle() }
            .popover(isPresented: $open, arrowEdge: .top) {
                EmojiPicker(categories: draft.store.composerEmoji) { item in
                    open = false
                    draft.replace(item.text, emojiID: item.id)
                }
            }
    }
}

/// Starts a mention at the caret, apart from the word before it.
struct MentionButton: View {
    let draft: RichDraft

    var body: some View {
        Button {
            let text = draft.composer.draft
            let prefix: String
            if let range = Range(draft.input.selection, in: text), let previous = text[..<range.lowerBound].last,
               !previous.isWhitespace && !"([{>".contains(previous) { prefix = " " } else { prefix = "" }
            draft.replace(prefix + "@")
        } label: {
            Text("@").font(.figtree(15, .semibold)).frame(width: 32, height: 32)
        }
        .buttonStyle(GhostButtonStyle())
        .help(L("ui.composer.mention_someone_2"))
    }
}

/// The mention picker floating just above the mention being typed, out of the layout around the editor.
/// It never takes the focus: the editor keeps the keys and moves in the list.
struct FloatingMentionPicker: ViewModifier {
    @Environment(Store.self) private var store
    let input: DraftInput
    @State private var panel = MentionPanel()

    func body(content: Content) -> some View {
        content
            .onChange(of: input.mention) { _, mention in
                guard let mention, let text = input.textView else { return panel.close() }
                panel.show(MentionPicker(draft: RichDraft(store: store, input: input)).environment(store),
                           above: mention.range, in: text)
            }
            .onDisappear { panel.close() }
    }
}

@MainActor
final class MentionPanel {
    static let width: CGFloat = 300
    private var panel: NSPanel?
    private var origin = NSPoint.zero
    private var resized: NSObjectProtocol?

    func show(_ content: some View, above range: NSRange, in text: NSTextView) {
        guard let window = text.window else { return close() }
        let line = text.firstRect(forCharacterRange: NSRange(location: range.location, length: 0), actualRange: nil)
        origin = NSPoint(x: line.minX, y: line.maxY + 4)
        // The list follows the query by itself; a new mention only moves it.
        if let panel { return panel.setFrameOrigin(origin) }
        let panel = NSPanel(contentRect: .zero, styleMask: [.borderless, .nonactivatingPanel], backing: .buffered, defer: true)
        panel.isOpaque = false
        panel.backgroundColor = .clear
        panel.hasShadow = true
        panel.becomesKeyOnlyIfNeeded = true
        // A new height keeps the top of the window: its bottom goes back above the line, so the list grows upwards.
        resized = NotificationCenter.default.addObserver(forName: NSWindow.didResizeNotification, object: panel, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated {
                guard let self, let panel = self.panel else { return }
                panel.setFrameOrigin(self.origin)
            }
        }
        let host = NSHostingController(rootView: content.frame(width: Self.width))
        host.sizingOptions = [.preferredContentSize]
        panel.contentViewController = host
        panel.appearance = window.effectiveAppearance
        panel.setFrameOrigin(origin)
        window.addChildWindow(panel, ordered: .above)
        self.panel = panel
    }

    func close() {
        guard let panel else { return }
        panel.parent?.removeChildWindow(panel)
        panel.orderOut(nil)
        if let resized { NotificationCenter.default.removeObserver(resized) }
        resized = nil
        self.panel = nil
    }
}

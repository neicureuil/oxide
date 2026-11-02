import AppKit
import SwiftUI
import OxideCore

struct Composer: View {
    @Environment(Store.self) private var store
    /// Only these from the shell: the editor is not redrawn with every shell change.
    let readOnly: Bool
    let replyOpen: Bool
    let isChannel: Bool
    @State private var emoji = false
    @State private var fun = false
    @State private var apps = false
    @State private var formatting = false
    @State private var height: CGFloat = 40
    @State private var input = DraftInput()
    @State private var dropping = false

    var body: some View {
        if readOnly {
            Muted(L("ui.composer.this_conversation_is_read_only")).padding(20)
                .frame(maxWidth: .infinity, alignment: .leading)
        } else {
            VStack(alignment: .leading, spacing: 6) {
                // Above the box rather than inside it, so the editor does not move while picking.
                if input.mention != nil { mentionPicker }
                box
            }
            .padding(EdgeInsets(top: 0, leading: 20, bottom: 16, trailing: 20))
        }
    }

    private var box: some View {
        VStack(alignment: .leading, spacing: 6) {
            if formatting { FormattingToolbar(input: input) }
            if composer.importance != .standard { ImportanceTag(importance: composer.importance) }
            if isChannel && !replyOpen {
                SyncedTextField(
                    placeholder: L("ui.composer.subject_optional"),
                    value: composer.subject,
                    onChange: { store.send(.subjectChanged(value: $0)) }
                )
                .textFieldStyle(.plain)
                .font(.figtree(13))
                .padding(.vertical, 3)
            }
            DraftEditor(
                placeholder: replyOpen ? L("ui.composer.reply_in_thread")
                    : isChannel ? L("ui.composer.start_a_post") : L("ui.composer.type_a_message"),
                draft: composer.draft,
                revision: composer.draftRev,
                cursor: Int(composer.draftCursor),
                mentions: composer.mentionRanges,
                formats: composer.draftFormats,
                expanded: formatting,
                input: input,
                height: $height,
                onChange: { store.send(.richDraftChanged(value: $0, formats: $1)) },
                onSend: { store.send(.send) },
                onMentionCommand: mentionCommand,
                onEditLast: editLast,
                takeFocusRequest: store.takeComposerFocusRequest
            )
            .frame(height: min(max(height, 40), 140))
            if let link = composer.linkPreview {
                LinkPreviewCard(preview: link) { store.send(.linkPreview(action: .dismiss)) }
            }
            if !composer.cards.isEmpty { ComposerCards(cards: composer.cards) }
            if !composer.attachments.isEmpty { AttachmentChips(attachments: composer.attachments) }
            tools
        }
        .onDrop(of: [.fileURL], isTargeted: $dropping) { providers in
            FilePanels.paths(from: providers) { paths in
                if !paths.isEmpty { store.send(.files(action: .attach(paths: paths))) }
            }
        }
        .onReceive(NotificationCenter.default.publisher(for: .focusComposer)) { _ in
            guard let text = input.textView, let window = text.window else { return }
            _ = store.takeComposerFocusRequest()
            window.makeFirstResponder(text)
            text.setSelectedRange(NSRange(location: (text.string as NSString).length, length: 0))
        }
        .onChange(of: input.linkRequested) { _, requested in if requested { formatting = true } }
        .onChange(of: input.mention) { _, mention in
            input.highlighted = 0
            if mention != nil { store.send(.mentionMembers) }
        }
        .padding(EdgeInsets(top: 10, leading: 12, bottom: 8, trailing: 12))
        .background(Theme.surface, in: RoundedRectangle(cornerRadius: 12))
        .overlay(RoundedRectangle(cornerRadius: 12).strokeBorder(dropping ? Theme.accent : Theme.borderStrong, lineWidth: dropping ? 2 : 1))
    }

    /// Sent with every core update; empty until the first one, which also brings the shell.
    private var composer: ComposerView {
        store.composer ?? ComposerView(
            sending: false, draft: "", draftRev: 0, draftCursor: 0,
            mentionRanges: [], draftFormats: [], mentionLoading: false, mentionError: nil,
            subject: "", importance: .standard, editText: "", attachments: [], cards: [], linkPreview: nil
        )
    }
    /// Text, files, or both; never while a file is still uploading.
    private var canSend: Bool {
        let text = !composer.draft.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        return !composer.sending && (text || !composer.attachments.isEmpty || !composer.cards.isEmpty)
            && composer.attachments.allSatisfy(\.ready)
    }
    private var people: [PersonView] {
        guard let mention = input.mention else { return [] }
        return Array(store.mentionPeople.filter {
            mention.query.isEmpty || ($0.name + " " + $0.email).localizedStandardContains(mention.query)
        }.prefix(8))
    }
    private var mentionPicker: some View {
        // Filtered once per drawing rather than once per row.
        let people = self.people
        return VStack(alignment: .leading, spacing: 2) {
            Text(L("ui.composer.mention_someone")).font(.figtree(11, .semibold)).foregroundStyle(Theme.muted).padding(6)
            ForEach(Array(people.enumerated()), id: \.element.mri) { index, person in
                Button { choose(person) } label: {
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
                    Button(L("ui.composer.try_again")) { store.send(.channelMembers) }
                }.padding(6).help(error)
            } else if people.isEmpty {
                Text(L("ui.composer.no_matching_participants")).font(.figtree(12)).foregroundStyle(Theme.muted).padding(6)
            }
        }
        .padding(4).background(Theme.surface, in: RoundedRectangle(cornerRadius: 12))
        .overlay(RoundedRectangle(cornerRadius: 12).strokeBorder(Theme.borderStrong))
    }
    private func replace(_ text: String, range: NSRange? = nil, mri: String? = nil, emojiID: String? = nil) {
        let selection = range ?? input.selection
        store.send(.replaceDraft(start: UInt32(selection.location), length: UInt32(selection.length), text: text, mention: mri, emoji: emojiID, style: input.currentFormat))
    }
    private func choose(_ person: PersonView) {
        guard let mention = input.mention else { return }
        input.dismiss(afterInsertion: true)
        replace("@\(person.name) ", range: mention.range, mri: person.mri)
    }
    /// What happens to the draft: being sent, waiting for the network, or how to send it.
    @ViewBuilder private var sendStatus: some View {
        if composer.sending {
            HStack(spacing: 6) {
                ProgressView().controlSize(.small)
                Text(L("ui.composer.sending")).font(.figtree(11, .medium)).foregroundStyle(Theme.muted)
            }
            .accessibilityElement(children: .combine)
        } else if !store.online {
            Label(L("ui.composer.offline_can_t_send"), systemImage: "wifi.slash")
                .font(.figtree(11, .medium))
                .foregroundStyle(Theme.errorText)
        } else if !composer.attachments.allSatisfy(\.ready) {
            Text(L("ui.composer.you_can_send_once_the_files")).font(.figtree(11)).foregroundStyle(Theme.subtle)
        } else {
            Text(formatting ? L("ui.composer.enter_to_send") : L("ui.composer.enter_to_send_2"))
                .font(.figtree(11))
                .foregroundStyle(Theme.subtle)
        }
    }
    /// `sendStatus` without words, for when there is no room for them.
    @ViewBuilder private var compactSendStatus: some View {
        if composer.sending {
            ProgressView().controlSize(.small).accessibilityLabel(L("ui.composer.sending"))
        } else if !store.online {
            Image(systemName: "wifi.slash")
                .foregroundStyle(Theme.errorText)
                .help(L("ui.composer.offline_can_t_send"))
                .accessibilityLabel(L("ui.composer.offline_can_t_send"))
        } else {
            Color.clear.frame(width: 0, height: 0)
        }
    }
    /// ↑ in an empty composer edits my latest message of the feed it writes to.
    private func editLast() -> Bool {
        let rows = replyOpen ? store.threadFeed : isChannel ? store.postFeed : store.chatFeed
        guard let message = Store.lastEditable(in: rows) else { return false }
        store.send(.edit(id: message.id))
        return true
    }
    private func mentionCommand(_ selector: Selector) -> Bool {
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
    /// "Set delivery options": Standard, Important, and Urgent in chats only.
    private var deliveryOptions: some View {
        Menu {
            Picker(L("ui.composer.delivery_options"), selection: Binding(
                get: { composer.importance },
                set: { store.send(.setImportance(importance: $0)) }
            )) {
                Text(L("ui.composer.standard")).tag(Importance.standard)
                Text(L("ui.composer.important")).tag(Importance.high)
                if !isChannel { Text(L("ui.composer.urgent")).tag(Importance.urgent) }
            }
            .pickerStyle(.inline)
        } label: {
            Image(systemName: "exclamationmark.circle")
                .font(.system(size: 16))
                .foregroundStyle(composer.importance == .standard ? Theme.muted : Theme.errorText)
                .frame(width: 32, height: 32)
        }
        .menuStyle(.borderlessButton)
        .menuIndicator(.hidden)
        .fixedSize()
        .help(L("ui.composer.delivery_options"))
        .accessibilityLabel(L("ui.composer.delivery_options"))
    }
    private var tools: some View {
        HStack(spacing: 2) {
            Button { formatting.toggle() } label: {
                Text("Aa").font(.figtree(13, .semibold)).frame(width: 32, height: 32)
            }
            .buttonStyle(GhostButtonStyle(accent: formatting))
            .background(formatting ? Theme.accentSoft : .clear, in: RoundedRectangle(cornerRadius: 6))
            .help(formatting ? L("ui.composer.hide_formatting_options") : L("ui.composer.show_formatting_options"))
            .accessibilityLabel(L("ui.composer.format"))
            IconButton(symbol: "face.smiling", help: L("ui.composer.add_an_emoji"), side: 32) { emoji.toggle() }
                .popover(isPresented: $emoji, arrowEdge: .top) {
                    EmojiPicker(categories: store.composerEmoji) { item in
                        emoji = false
                        replace(item.text, emojiID: item.id)
                    }
                }
            Button { fun.toggle() } label: {
                Text("GIF").font(.figtree(11, .semibold)).frame(width: 32, height: 32)
            }
            .buttonStyle(GhostButtonStyle())
            .help(L("ui.fun_picker.gifs_and_stickers"))
            .accessibilityLabel(L("ui.fun_picker.gifs_and_stickers"))
            .popover(isPresented: $fun, arrowEdge: .top) { FunPicker { fun = false } }
            IconButton(symbol: "square.grid.2x2", help: L("ui.extensions.apps"), side: 32) { apps.toggle() }
                .popover(isPresented: $apps, arrowEdge: .top) { ExtensionsFlyout { apps = false } }
            Button {
                let prefix: String
                if let range = Range(input.selection, in: composer.draft), let previous = composer.draft[..<range.lowerBound].last,
                   !previous.isWhitespace && !"([{>".contains(previous) { prefix = " " } else { prefix = "" }
                replace(prefix + "@")
            } label: {
                Text("@").font(.figtree(15, .semibold)).frame(width: 32, height: 32)
            }
            .buttonStyle(GhostButtonStyle())
            .help(L("ui.composer.mention_someone_2"))
            IconButton(symbol: "paperclip", help: L("ui.composer.attach_files"), side: 32) {
                let paths = FilePanels.chooseFiles(prompt: L("ui.composer.attach"))
                if !paths.isEmpty { store.send(.files(action: .attach(paths: paths))) }
            }
            deliveryOptions
            // On one line or not at all: the narrow thread pane would otherwise wrap it letter by letter.
            ViewThatFits(in: .horizontal) {
                sendStatus.fixedSize()
                compactSendStatus
            }
            .frame(maxWidth: .infinity, alignment: .trailing)
            .padding(.trailing, 8)
            Button {
                store.send(.send)
            } label: {
                Image(systemName: "paperplane.fill").frame(width: 36, height: 32)
            }
            .buttonStyle(PrimaryButtonStyle())
            .disabled(!canSend)
            .help(formatting ? L("ui.composer.send_enter") : L("ui.composer.send_shift_enter_for_a_new"))
            .accessibilityLabel(L("ui.composer.send"))
        }
    }
}

/// Native text view, for AppKit's spell checking, dictation, emoji palette and input methods; Return sends.
private struct DraftEditor: NSViewRepresentable {
    let placeholder: String
    let draft: String
    let revision: UInt64
    let cursor: Int
    let mentions: [MentionRange]
    let formats: [ComposerFormat]
    let expanded: Bool
    let input: DraftInput
    @Binding var height: CGFloat
    let onChange: (String, [ComposerFormat]) -> Void
    let onSend: () -> Void
    let onMentionCommand: (Selector) -> Bool
    /// ↑ with an empty draft; false when there is nothing to edit.
    let onEditLast: () -> Bool
    /// Whether a « Répondre » asked for the focus before this editor existed (a thread opening).
    let takeFocusRequest: () -> Bool

    func makeCoordinator() -> Coordinator { Coordinator(self) }

    func makeNSView(context: Context) -> NSScrollView {
        let scroll = NSScrollView()
        let storage = NSTextStorage()
        let layout = ComposerLayoutManager()
        let container = NSTextContainer(containerSize: NSSize(width: 400, height: CGFloat.greatestFiniteMagnitude))
        container.widthTracksTextView = true
        storage.addLayoutManager(layout)
        layout.addTextContainer(container)
        let text = ComposerTextView(frame: NSRect(x: 0, y: 0, width: 400, height: 40), textContainer: container)
        text.isVerticallyResizable = true
        text.isHorizontallyResizable = false
        text.autoresizingMask = [.width]
        text.maxSize = NSSize(width: CGFloat.greatestFiniteMagnitude, height: CGFloat.greatestFiniteMagnitude)
        scroll.documentView = text
        input.textView = text
        text.formatShortcut = { [weak input] key in input?.shortcut(key) }
        text.sendShortcut = { [weak coordinator = context.coordinator] in
            guard let coordinator else { return }
            if !coordinator.parent.onMentionCommand(#selector(NSResponder.insertNewline(_:))) { coordinator.parent.onSend() }
        }
        scroll.drawsBackground = false
        scroll.hasVerticalScroller = true
        scroll.autohidesScrollers = true
        text.delegate = context.coordinator
        text.drawsBackground = false
        text.isRichText = true
        text.importsGraphics = false
        text.isAutomaticLinkDetectionEnabled = true
        text.allowsUndo = true
        text.isContinuousSpellCheckingEnabled = true
        text.restoreAutomaticSpellingCorrection()
        text.isAutomaticQuoteSubstitutionEnabled = false
        text.textContainerInset = NSSize(width: 0, height: 4)
        text.textContainer?.lineFragmentPadding = 2
        text.font = ComposerText.baseFont
        text.textColor = .labelColor
        text.textStorage?.setAttributedString(ComposerText.attributed(draft, formats: formats))
        text.setSelectedRange(NSRange(location: (draft as NSString).length, length: 0))
        text.setAccessibilityLabel(placeholder)
        DispatchQueue.main.async { self.input.capture(draft, selection: NSRange(location: (draft as NSString).length, length: 0), mentions: mentions, coreChange: true) }
        context.coordinator.revision = revision
        context.coordinator.install(placeholder: placeholder, on: text)
        if takeFocusRequest() {
            DispatchQueue.main.async { text.window?.makeFirstResponder(text) }
        }
        return scroll
    }

    func updateNSView(_ scroll: NSScrollView, context: Context) {
        let coordinator = context.coordinator
        coordinator.parent = self
        guard let text = scroll.documentView as? NSTextView else { return }
        coordinator.placeholder?.stringValue = placeholder
        // Only replace the text when the core changed it (selection, sent, emoji…).
        if coordinator.revision != revision {
            coordinator.revision = revision
            coordinator.applyingCore = true
            coordinator.colored = nil // Replacing the text drops its mention colors.
            text.textStorage?.setAttributedString(ComposerText.attributed(draft, formats: formats))
            text.setSelectedRange(NSRange(location: min(cursor, (draft as NSString).length), length: 0))
            coordinator.applyingCore = false
            let selected = text.selectedRange()
            let value = text.string
            DispatchQueue.main.async { self.input.capture(value, selection: selected, mentions: mentions, coreChange: true) }
            coordinator.refresh(text)
            // Keeps the caret here after a popover insertion, without stealing focus from another control.
            if let window = text.window, window.firstResponder == nil || window.firstResponder === window {
                window.makeFirstResponder(text)
            }
        }
        if coordinator.styledMentions != mentions {
            coordinator.styledMentions = mentions
            let value = text.string
            let selected = text.selectedRange()
            DispatchQueue.main.async { self.input.capture(value, selection: selected, mentions: mentions) }
        }
        // Recolor mentions only when the text or the mentions changed, not on every redraw.
        if text.string == draft, coordinator.colored?.draft != draft || coordinator.colored?.mentions != mentions,
           let layout = text.layoutManager {
            coordinator.colored = (draft, mentions)
            layout.removeTemporaryAttribute(.foregroundColor, forCharacterRange: NSRange(location: 0, length: (draft as NSString).length))
            for mention in mentions {
                let range = NSRange(location: Int(mention.start), length: Int(mention.length))
                if NSMaxRange(range) <= (draft as NSString).length { layout.addTemporaryAttribute(.foregroundColor, value: NSColor.controlAccentColor, forCharacterRange: range) }
            }
        }
    }

    final class Coordinator: NSObject, NSTextViewDelegate {
        var parent: DraftEditor
        var revision: UInt64 = 0
        var applyingCore = false
        var styledMentions: [MentionRange] = []
        var colored: (draft: String, mentions: [MentionRange])?
        var placeholder: NSTextField?
        init(_ parent: DraftEditor) { self.parent = parent }

        func install(placeholder value: String, on text: NSTextView) {
            let label = NSTextField(labelWithString: value)
            label.font = text.font
            label.textColor = .placeholderTextColor
            label.translatesAutoresizingMaskIntoConstraints = false
            text.addSubview(label)
            NSLayoutConstraint.activate([
                label.leadingAnchor.constraint(equalTo: text.leadingAnchor, constant: 2),
                label.topAnchor.constraint(equalTo: text.topAnchor, constant: 4),
            ])
            placeholder = label
            refresh(text)
        }
        func refresh(_ text: NSTextView) {
            placeholder?.isHidden = !text.string.isEmpty
            if let container = text.textContainer, let layout = text.layoutManager {
                layout.ensureLayout(for: container)
                let used = layout.usedRect(for: container).height + text.textContainerInset.height * 2
                if abs(used - parent.height) > 0.5 {
                    DispatchQueue.main.async { self.parent.height = used }
                }
            }
        }
        func textDidChange(_ notification: Notification) {
            guard let text = notification.object as? NSTextView else { return }
            refresh(text)
            parent.onChange(text.string, ComposerText.formats(text))
            parent.input.capture(text.string, selection: text.selectedRange(), mentions: parent.mentions)
        }
        func textViewDidChangeSelection(_ notification: Notification) {
            guard let text = notification.object as? NSTextView, !applyingCore else { return }
            let selection = text.selectedRange()
            let value = text.string
            let currentRevision = revision
            DispatchQueue.main.async {
                guard self.revision == currentRevision else { return }
                self.parent.input.capture(value, selection: selection, mentions: self.parent.mentions)
            }
        }
        func textView(_ textView: NSTextView, doCommandBy selector: Selector) -> Bool {
            if !textView.hasMarkedText() && NSApp.currentEvent?.modifierFlags.contains(.shift) != true && parent.onMentionCommand(selector) { return true }
            if selector == #selector(NSResponder.moveUp(_:)), textView.string.isEmpty, !textView.hasMarkedText() {
                return parent.onEditLast()
            }
            if selector == #selector(NSResponder.insertNewline(_:)) {
                if NSApp.currentEvent?.modifierFlags.contains(.shift) == true || (parent.expanded && NSApp.currentEvent?.modifierFlags.contains(.command) != true) {
                    textView.insertNewlineIgnoringFieldEditor(nil)
                } else if !textView.hasMarkedText() {
                    parent.onSend()
                }
                return true
            }
            return false
        }
    }
}

import SwiftUI
import OxideCore

struct MessageRow: View {
    @Environment(Store.self) private var store
    let message: MessageView
    let bubble: Bool
    /// Answers this message: opens the post's thread, or quotes it in the composer.
    var reply: (() -> Void)?
    /// Only the hovered message shows its action bar; the context menu and VoiceOver actions are always there.
    @State private var hovered = false
    /// The reaction picker is the view's own: opening it changes nothing in the core.
    @State private var pickerOpen = false
    @State private var confirmDelete = false

    var body: some View {
        Group {
            if !bubble {
                // Channel posts and thread replies: every message says who wrote it.
                HStack(alignment: .top, spacing: 10) {
                    Avatar(name: message.sender, id: message.senderId, size: 32, shape: message.senderAvatarShape,
                           presence: message.senderPresence)
                    VStack(alignment: .leading, spacing: 4) {
                        meta
                        content
                        reactionBar
                    }
                    .frame(maxWidth: .infinity, alignment: .leading)
                }
            } else if message.mine {
                HStack {
                    Spacer(minLength: 80)
                    VStack(alignment: .trailing, spacing: 4) {
                        meta
                        bubbleCard
                        reactionBar
                    }
                    .frame(maxWidth: 560, alignment: .trailing)
                }
            } else {
                HStack(alignment: .top, spacing: 10) {
                    Avatar(name: message.sender, id: message.senderId, size: 32, shape: message.senderAvatarShape,
                           presence: message.senderPresence)
                    VStack(alignment: .leading, spacing: 4) {
                        meta
                        bubbleCard
                        reactionBar
                    }
                    .frame(maxWidth: 600, alignment: .leading)
                    Spacer(minLength: 40)
                }
            }
        }
        .contentShape(Rectangle())
        .onHover { hovered = $0 }
        .contextMenu { if !message.editing { menu } }
        .alert(L("ui.message_row.delete_this_message"), isPresented: $confirmDelete) {
            Button(L("ui.message_row.delete"), role: .destructive) { store.send(.delete(id: message.id)) }
            Button(L("ui.message_row.cancel"), role: .cancel) {}
        } message: {
            Text(L("ui.message_row.it_will_be_replaced_with_this"))
        }
        // Table cells are reused for other messages.
        .onChange(of: message.id) {
            pickerOpen = false
            confirmDelete = false
        }
    }

    private var canEdit: Bool { message.editable && !message.editing }
    private var text: String { message.runs.map(\.text).joined() }

    /// Everything the hover bar offers, and more, for its "more" button and the right click.
    @ViewBuilder private var menu: some View {
        if !message.deleted {
            if message.canReact {
                Button(L("ui.message_row.react")) { pickerOpen = true }
            }
            if let reply { Button(L("ui.message_row.reply"), action: reply) }
            if !text.isEmpty { Button(L("ui.message_row.copy_text")) { copy() } }
            if canEdit {
                Divider()
                Button(L("ui.message_row.edit")) { store.send(.edit(id: message.id)) }
                Button(L("ui.message_row.delete_2"), role: .destructive) { confirmDelete = true }
            }
        }
    }

    /// The menu's commands as VoiceOver actions, plus the message's links.
    @ViewBuilder private var accessibilityCommands: some View {
        if message.canReact {
            Button(L("ui.message_row.react")) { pickerOpen = true }
        }
        if let reply { Button(L("ui.message_row.reply"), action: reply) }
        Button(L("ui.message_row.copy_text")) { copy() }
        if canEdit {
            Button(L("ui.message_row.edit")) { store.send(.edit(id: message.id)) }
            Button(L("ui.message_row.delete")) { confirmDelete = true }
        }
        ForEach(Array(Set(message.runs.compactMap(\.link))).sorted(), id: \.self) { link in
            Button(L("ui.message_row.open_link", ["link": link])) { store.send(.openUrl(url: link)) }
        }
    }

    private func copy() { NSPasteboard.copy(text) }

    /// One VoiceOver element per message: who, when, what, and its reactions.
    private var spokenLabel: String {
        var parts = [message.mine ? L("ui.message_row.you") : message.sender, message.time]
        if message.importance != .standard { parts.append(ImportanceTag.label(message.importance)) }
        if let subject = message.subject { parts.append(subject) }
        parts.append(message.deleted ? L("ui.message_row.this_message_has_been_deleted") : text)
        if !message.images.isEmpty {
            parts.append(message.images.count == 1 ? L("ui.message_row.1_image") : L("ui.message_row.images", ["imagesCount": message.images.count]))
        }
        if !message.files.isEmpty {
            parts.append(message.files.count == 1 ? L("ui.message_row.1_file") : L("ui.message_row.files", ["filesCount": message.files.count]))
        }
        for reaction in message.reactions { parts.append("\(reaction.emoji) \(reaction.count)") }
        if message.pending { parts.append(L("ui.message_row.sending_reaction")) }
        return parts.filter { !$0.isEmpty }.joined(separator: ", ")
    }

    private var bubbleCard: some View {
        content
            .padding(.horizontal, 14)
            .padding(.vertical, 10)
            .background(message.mine ? Theme.mineBubble : Theme.surface, in: bubbleShape)
            .overlay(bubbleShape.strokeBorder(message.mine ? .clear : Theme.bubbleBorder))
    }
    private var bubbleShape: UnevenRoundedRectangle {
        message.mine
            ? UnevenRoundedRectangle(topLeadingRadius: 12, bottomLeadingRadius: 12, bottomTrailingRadius: 12, topTrailingRadius: 4)
            : UnevenRoundedRectangle(topLeadingRadius: 4, bottomLeadingRadius: 12, bottomTrailingRadius: 12, topTrailingRadius: 12)
    }

    /// « **Nom** · 20:12 » (only the time on my bubbles), carrying the actions so showing them never moves the feed.
    private var meta: some View {
        HStack(spacing: 8) {
            if message.mine && bubble {
                actions
                // Spoken with the message itself.
                MessageAuthorLine(text: authorLine, messageId: message.id).accessibilityHidden(true)
                if let receipt = message.receipt { ReceiptIcon(receipt: receipt) }
            } else {
                MessageAuthorLine(text: authorLine, messageId: message.id).accessibilityHidden(true)
                actions
            }
        }
        .frame(height: 24)
    }

    /// « **Nom** · 20:12 », or only the time on my bubbles: text, so a selection takes it along.
    private var authorLine: NSAttributedString {
        Self.authorLine(message, bubble: bubble)
    }

    static func authorLine(_ message: MessageView, bubble: Bool) -> NSAttributedString {
        let time = NSAttributedString(string: message.time, attributes: [
            .font: MessageRichText.font(size: 12), .foregroundColor: NSColor(Theme.muted),
        ])
        guard !(message.mine && bubble) else { return time }
        let line = NSMutableAttributedString(string: message.mine ? L("ui.message_row.you") : message.sender, attributes: [
            .font: MessageRichText.font(size: bubble ? 12 : 13, bold: true), .foregroundColor: NSColor(Theme.text),
        ])
        line.append(NSAttributedString(string: " · ", attributes: [
            .font: MessageRichText.font(size: 12), .foregroundColor: NSColor(Theme.muted),
        ]))
        line.append(time)
        return line
    }

    @ViewBuilder private var actions: some View {
        let visible = hovered || message.pending || pickerOpen
        if visible && !message.deleted {
            HStack(spacing: 2) {
                if message.canReact || message.pending {
                    Button {
                        pickerOpen.toggle()
                    } label: {
                        Group {
                            if message.pending {
                                ProgressView().controlSize(.mini)
                            } else {
                                Image(systemName: "face.smiling")
                            }
                        }
                        .frame(width: 24, height: 24)
                    }
                    .buttonStyle(GhostButtonStyle())
                    .disabled(message.pending)
                    .help(message.pending ? L("ui.message_row.sending_reaction_2") : L("ui.message_row.react"))
                    .accessibilityLabel(message.pending ? L("ui.message_row.sending_reaction_3") : L("ui.message_row.react"))
                    .popover(isPresented: $pickerOpen, arrowEdge: .top) {
                        EmojiPicker(categories: store.composerEmoji) { item in
                            // Keep an existing wire key so selecting our reaction again removes it.
                            let mine = message.reactions.first {
                                $0.mine && $0.emojiId == item.id
                            }
                            let key = mine?.key ?? item.id
                            pickerOpen = false
                            store.send(.react(id: message.id, key: key))
                        }
                        .disabled(!message.canReact)
                    }
                }
                if let reply, !message.pending {
                    IconButton(symbol: "arrowshape.turn.up.left", help: L("ui.message_row.reply"), side: 24, glyph: 13, action: reply)
                }
                if canEdit {
                    IconButton(symbol: "pencil", help: L("ui.message_row.edit"), side: 24, glyph: 13) {
                        store.send(.edit(id: message.id))
                    }
                    IconButton(symbol: "trash", help: L("ui.message_row.delete"), side: 24, glyph: 13) {
                        confirmDelete = true
                    }
                }
                // A right click on selectable text opens the text's menu, not the message's: this one always can.
                if !message.editing {
                    Menu { menu } label: {
                        Image(systemName: "ellipsis").font(.system(size: 13)).frame(width: 24, height: 24)
                    }
                    .menuStyle(.borderlessButton).menuIndicator(.hidden).fixedSize().pointerCursor()
                    .help(L("ui.message_row.more_actions"))
                    .accessibilityLabel(L("ui.message_row.more_actions"))
                }
            }
        }
    }

    @ViewBuilder private var content: some View {
        let caption = ImageCaption(sender: message.mine ? L("ui.message_row.you") : message.sender, time: message.time)
        VStack(alignment: message.mine && bubble ? .trailing : .leading, spacing: 8) {
            if message.importance != .standard { ImportanceTag(importance: message.importance) }
            if let subject = message.subject {
                Text(subject).font(.figtree(16, .semibold))
            }
            if message.editing {
                editor
            } else if message.deleted {
                Muted(L("ui.message_row.this_message_has_been_deleted"), size: 14)
                    .accessibilityLabel(spokenLabel)
            } else {
                MessageBody(blocks: message.blocks, size: MessageBody.emojiOnly(message.blocks) ? 30 : 14)
                    .environment(\.cardMessageId, message.id)
                    .font(.figtree(14))
                    .textSelection(.enabled)
                    .environment(\.openURL, OpenURLAction { url in
                        store.send(.openUrl(url: url.absoluteString))
                        return .handled
                    })
                    .accessibilityElement(children: .ignore)
                    .accessibilityLabel(spokenLabel)
                    .accessibilityActions { accessibilityCommands }
            }
            // Images the body does not place itself (file previews, images in tables).
            let inline = Set(MessageBody.images(message.blocks).map(\.src))
            ForEach(message.images.filter { !inline.contains($0.src) }, id: \.src) { PreviewImage(image: $0) }
            ForEach(message.files, id: \.key) { FileCard(file: $0).frame(width: 340) }
            if let link = message.linkPreview { LinkPreviewCard(preview: link).frame(width: 340) }
        }
        .environment(\.imageCaption, caption)
    }

    private var editor: some View {
        MessageEditor(id: message.id)
    }

    @ViewBuilder private var reactionBar: some View {
        if !message.deleted && !message.reactions.isEmpty {
            HStack(spacing: 6) {
                ForEach(message.reactions, id: \.key) { reaction in
                    Button {
                        store.send(.react(id: message.id, key: reaction.key))
                    } label: {
                        HStack(spacing: 4) {
                            TeamsEmoji(id: reaction.emojiId, text: reaction.emoji, size: 16)
                            Text("\(reaction.count)").font(.figtree(12, reaction.mine ? .semibold : .regular))
                        }
                        .padding(.horizontal, 8)
                        .frame(height: 24)
                        .background(reaction.mine ? Theme.accentSoft : Theme.surface, in: Capsule())
                        .overlay(Capsule().strokeBorder(reaction.mine ? Theme.accent : Theme.border))
                    }
                    .buttonStyle(.plain).pointerCursor()
                    .disabled(!message.canReact)
                    .help(reaction.hint)
                }
            }
        }
    }
}

/// My message in the composer's rich editor, inside its bubble: Return saves, Shift-Return breaks the line, Escape cancels.
private struct MessageEditor: View {
    @Environment(Store.self) private var store
    let id: String
    @State private var input = DraftInput()
    @State private var height: CGFloat = 0
    @State private var formatting = false
    @State private var toolbarHeight: CGFloat = 0
    private static let spacing: CGFloat = 8

    var body: some View {
        let draft = RichDraft(store: store, input: input)
        let composer = draft.composer
        VStack(alignment: .leading, spacing: Self.spacing) {
            if formatting {
                FormattingToolbar(input: input).onGeometryChange(for: CGFloat.self) { $0.size.height } action: { toolbarHeight = $0 }
            }
            DraftHeight(draft: composer.draft, formats: composer.draftFormats) {
                RichDraftEditor(
                    placeholder: L("ui.message_row.edit_message"),
                    input: input,
                    formatting: $formatting,
                    height: $height,
                    onSend: { store.send(.saveEdit) },
                    onCommand: { selector in
                        guard selector == #selector(NSResponder.cancelOperation(_:)) else { return false }
                        store.send(.cancelEdit)
                        return true
                    },
                    takeFocusRequest: { true }
                )
                .modifier(FloatingMentionPicker(input: input))
            }
            .padding(.horizontal, 8)
            .padding(.vertical, 4)
            .background(Theme.surface, in: RoundedRectangle(cornerRadius: 8))
            .overlay(RoundedRectangle(cornerRadius: 8).strokeBorder(Theme.borderStrong))
            // Pasted images show in the text; a failed upload needs its chip to say so.
            if !failed.isEmpty {
                AttachmentChips(attachments: failed)
            }
            HStack(spacing: 8) {
                HStack(spacing: 2) {
                    FormattingToggle(formatting: $formatting)
                    EmojiButton(draft: draft)
                    MentionButton(draft: draft)
                }
                Button(L("ui.message_row.save")) { store.send(.saveEdit) }.buttonStyle(PrimaryButtonStyle())
                Button(L("ui.message_row.cancel")) { store.send(.cancelEdit) }
                    .buttonStyle(OutlineButtonStyle())
                    .keyboardShortcut(.cancelAction)
                Muted(formatting ? L("ui.message_row.enter_to_save") : L("ui.message_row.enter_to_save_enter_for_a"), size: 11)
            }
        }
        // The feed sizes rows once, with an off-screen copy that has no window and none of this view's state:
        // a taller text, the formatting tools or a failed upload ask for the row again.
        .onChange(of: height) { resize() }
        .onChange(of: extra) { resize() }
        .onChange(of: failed) { resize() }
        .onDisappear { if extra > 0 { NotificationCenter.default.post(name: .feedRowResized, object: FeedRowResize(id: id, extra: 0)) } }
    }
    private var failed: [PendingFileView] {
        (store.composer ?? .empty).attachments.filter { $0.error != nil }
    }
    /// What the off-screen copy cannot measure; it does see the store, so the failed uploads' chips are in its height.
    private var extra: CGFloat {
        formatting ? toolbarHeight + Self.spacing : 0
    }
    private func resize() {
        guard input.textView?.window != nil else { return }
        NotificationCenter.default.post(name: .feedRowResized, object: FeedRowResize(id: id, extra: extra))
    }
}

/// The editor at its text's height for the width offered, computed rather than read from the view so the feed's off-screen measure agrees.
private struct DraftHeight: Layout {
    let draft: String
    let formats: [ComposerFormat]
    static let maxHeight: CGFloat = 400

    func sizeThatFits(proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) -> CGSize {
        let width = proposal.width.flatMap { $0.isFinite ? $0 : nil } ?? 520
        let text = ComposerText.height(draft, formats: formats, width: width)
        return CGSize(width: width, height: min(max(text, 40), Self.maxHeight))
    }
    func placeSubviews(in bounds: CGRect, proposal: ProposedViewSize, subviews: Subviews, cache: inout ()) {
        for view in subviews { view.place(at: bounds.origin, proposal: ProposedViewSize(bounds.size)) }
    }
}

/// A fixed box whether loaded or not, so rows never resize: the declared size capped like the web client, else 420×250.
struct PreviewImage: View {
    @Environment(Store.self) private var store
    @Environment(\.imageCaption) private var caption
    let image: ImageView
    var height: CGFloat = 250
    var savable = true

    static let maxSize = CGSize(width: 620, height: 480)
    /// The declared size, scaled down to fit `maxSize`.
    static func declaredSize(_ image: ImageView) -> CGSize? {
        guard let w = image.width, let h = image.height, w > 0, h > 0 else { return nil }
        let (width, height) = (CGFloat(w), CGFloat(h))
        let scale = min(1, maxSize.width / width, maxSize.height / height)
        return CGSize(width: width * scale, height: height * scale)
    }
    /// Only a height, as a card's `"height": "24px"` badge: its width follows the picture.
    static func declaredHeight(_ image: ImageView) -> CGFloat? {
        guard image.width == nil, let h = image.height, h > 0 else { return nil }
        return min(CGFloat(h), maxSize.height)
    }
    /// An own width, not a flexible one: in an `auto` card column, a flexible image would squeeze its neighbours to nothing.
    /// Square until the picture loads, then its aspect ratio, capped at `maxSize`.
    static func width(atHeight height: CGFloat, of picture: NSImage?) -> CGFloat {
        guard let size = picture?.size, size.width > 0, size.height > 0 else { return height }
        return min(height * size.width / size.height, maxSize.width)
    }
    /// Height in a column `width` wide, for the feed's first estimate.
    static func estimatedHeight(_ image: ImageView, width: CGFloat) -> CGFloat {
        if let height = declaredHeight(image) { return height }
        guard let size = declaredSize(image) else { return 250 }
        return size.height * min(1, width / size.width)
    }

    /// A card's avatar or icon: its exact size, and only a glyph if it fails.
    private var small: Bool {
        if let height = Self.declaredHeight(image) { return height <= 160 }
        return Self.declaredSize(image).map { $0.width <= 160 && $0.height <= 160 } ?? false
    }

    var body: some View {
        if let size = Self.declaredSize(image), small {
            box.frame(width: size.width, height: size.height)
        } else if let size = Self.declaredSize(image) {
            box
                .aspectRatio(size.width / size.height, contentMode: .fit)
                .frame(maxWidth: size.width, alignment: .leading)
        } else if let height = Self.declaredHeight(image) {
            box.frame(width: Self.width(atHeight: height, of: store.imageBox(image.src).image), height: height)
        } else {
            box.frame(maxWidth: 420).frame(height: height)
        }
    }
    private func zoom(_ picture: NSImage) {
        store.zoomedImage = ZoomedImage(picture: picture, alt: image.alt, caption: caption, savable: savable)
    }
    /// The alt text is kept for VoiceOver only: Teams fills it with « image » or a file name.
    private var box: some View {
        ZStack(alignment: .topLeading) {
            // Placeholder only: a loaded image keeps the box but not its fill.
            if case .ready = image.status, store.imageBox(image.src).image != nil {} else {
                RoundedRectangle(cornerRadius: 8).fill(Theme.input)
            }
            switch image.status {
            case .ready:
                if let picture = store.imageBox(image.src).image {
                    Image(nsImage: picture)
                        .resizable()
                        .interpolation(.medium)
                        .scaledToFit()
                        .clipShape(RoundedRectangle(cornerRadius: 8))
                        .onTapGesture { zoom(picture) }
                        .pointerCursor()
                        .help(L("ui.message_row.enlarge_image"))
                        .frame(maxWidth: .infinity, maxHeight: .infinity, alignment: .topLeading)
                        .accessibilityLabel(image.alt)
                        .accessibilityAddTraits(.isButton)
                        .accessibilityAction { zoom(picture) }
                }
            case .loading:
                ProgressView()
                    .controlSize(.small)
                    .frame(maxWidth: .infinity, maxHeight: .infinity)
                    .accessibilityLabel(L("ui.message_row.loading_image"))
            case .failed(let reason) where small:
                Button { store.send(.retryImage(src: image.src)) } label: {
                    Image(systemName: "photo").foregroundStyle(Theme.muted)
                        .frame(maxWidth: .infinity, maxHeight: .infinity)
                }
                .buttonStyle(.plain)
                .help("\(reason) · \(L("ui.message_row.retry_image"))")
                .accessibilityLabel(L("ui.message_row.retry_image"))
            case .failed(let reason):
                VStack(alignment: .leading, spacing: 6) {
                    Muted(reason, size: 11)
                    Button(L("ui.message_row.retry_image")) { store.send(.retryImage(src: image.src)) }
                        .buttonStyle(GhostButtonStyle())
                        .font(.figtree(12))
                }
                .padding(10)
            }
        }
    }
}

/// Teams' default alt texts say nothing: only a real name is worth showing.
private func meaningful(_ alt: String) -> Bool {
    !["image", "images", "image partagée"].contains(alt.lowercased())
}

private extension NSImage {
    var pngData: Data? {
        tiffRepresentation.flatMap(NSBitmapImageRep.init(data:))?.representation(using: .png, properties: [:])
    }
}

/// Who sent a message's images and when, for their full-size view; unset outside a message.
struct ImageCaption {
    let sender: String
    let time: String
}
private struct ImageCaptionKey: EnvironmentKey { static let defaultValue: ImageCaption? = nil }
extension EnvironmentValues {
    var imageCaption: ImageCaption? {
        get { self[ImageCaptionKey.self] }
        set { self[ImageCaptionKey.self] = newValue }
    }
}

/// A message image shown full size over the window.
struct ZoomedImage {
    let picture: NSImage
    let alt: String
    let caption: ImageCaption?
    /// False for a file's thumbnail, which is not the file.
    let savable: Bool
}

/// Full-size view of a message image over the dimmed window, as Teams shows it: a bar naming who sent it, with the
/// actions, above the image fitted to the rest of the window. Escape, the back arrow or a click around the image
/// closes it.
struct ImageViewer: View {
    @Environment(Store.self) private var store
    let image: ZoomedImage
    /// Held so typing and Return do not reach the composer hidden underneath.
    @FocusState private var focused: Bool
    /// Scale over the fitted size, around the image's centre.
    @State private var zoom: CGFloat = 1
    @State private var zoomBeforePinch: CGFloat = 1
    @State private var offset: CGSize = .zero
    @State private var offsetBeforeDrag: CGSize = .zero
    /// The whole viewer and the image's area below its bar, in window coordinates, for the wheel.
    @State private var frame: CGRect = .zero
    @State private var area: CGRect = .zero
    @State private var wheel: Any?
    /// Opened by a click or VoiceOver action in it, so the key window when it appears.
    @State private var window: NSWindow?
    private static let zooms: ClosedRange<CGFloat> = 0.25...8
    private static let step: CGFloat = 1.5
    private var picture: NSImage { image.picture }
    private var alt: String { image.alt }

    var body: some View {
        VStack(spacing: 0) {
            bar
            GeometryReader { area in
                let size = Self.fittedSize(of: picture.size, in: area.size)
                ZStack {
                    Color.black.opacity(0.55)
                        .contentShape(Rectangle())
                        .onTapGesture(perform: close)
                        .accessibilityHidden(true)
                    Image(nsImage: picture)
                        .resizable()
                        .interpolation(.high)
                        .frame(width: size.width, height: size.height)
                        .scaleEffect(zoom)
                        .offset(offset)
                        .gesture(pan, including: zoom > 1 ? .all : .none)
                        .contextMenu {
                            Button(L("ui.message_row.copy_image"), action: copy)
                            if image.savable { Button(L("ui.message_row.save_image"), action: save) }
                        }
                        .accessibilityLabel(alt)
                }
                .frame(width: area.size.width, height: area.size.height)
                .clipped()
                .gesture(pinch)
            }
            .onGeometryChange(for: CGRect.self, of: { $0.frame(in: .global) }) { area = $0 }
        }
        .background {
            Button("") { setZoom(1) }.keyboardShortcut("0").hidden()
        }
        .onGeometryChange(for: CGRect.self, of: { $0.frame(in: .global) }) { frame = $0 }
        .focusable()
        .focusEffectDisabled()
        .focused($focused)
        .onAppear {
            focused = true
            window = NSApp.keyWindow
            wheel = NSEvent.addLocalMonitorForEvents(matching: .scrollWheel, handler: zoom(by:))
        }
        .onDisappear { if let wheel { NSEvent.removeMonitor(wheel) } }
    }

    private var bar: some View {
        HStack(spacing: 6) {
            IconButton(symbol: "arrow.left", help: L("ui.message_row.close"), side: 30, glyph: 14, action: close)
                .keyboardShortcut(.cancelAction)
            VStack(alignment: .leading, spacing: 2) {
                Text(image.caption?.sender ?? (meaningful(alt) ? alt : L("ui.message_row.image")))
                    .font(.figtree(13, .semibold))
                    .foregroundStyle(Theme.text)
                    .lineLimit(1)
                if let subtitle { Muted(subtitle, size: 11).lineLimit(1) }
            }
            .padding(.leading, 6)
            Spacer()
            if zoom != 1 || offset != .zero {
                IconButton(symbol: "scope", help: L("ui.message_row.fit_image"), side: 30, glyph: 14) { setZoom(1) }
            }
            IconButton(symbol: "minus.magnifyingglass", help: L("ui.message_row.zoom_out"), side: 30, glyph: 14) {
                setZoom(zoom / Self.step)
            }
                .keyboardShortcut("-")
                .disabled(zoom <= Self.zooms.lowerBound)
            IconButton(symbol: "plus.magnifyingglass", help: L("ui.message_row.zoom_in"), side: 30, glyph: 14) {
                setZoom(zoom * Self.step)
            }
                .keyboardShortcut("=")
                .disabled(zoom >= Self.zooms.upperBound)
            Divider().frame(height: 18).padding(.horizontal, 4)
            IconButton(symbol: "doc.on.doc", help: L("ui.message_row.copy_image"), side: 30, glyph: 14, action: copy)
            if image.savable {
                IconButton(symbol: "arrow.down.to.line", help: L("ui.message_row.save_image"), side: 30, glyph: 14, action: save)
            }
        }
        .padding(.horizontal, 12)
        .frame(height: 56)
        .background(Theme.surface)
        .overlay(alignment: .bottom) { Divider().overlay(Theme.border) }
    }

    /// « image | 10:00 » under the sender, as Teams words it.
    private var subtitle: String? {
        image.caption.map { [meaningful(alt) ? alt : L("ui.message_row.image"), $0.time].joined(separator: " | ") }
    }

    private var pan: some Gesture {
        DragGesture()
            .onChanged { drag in
                offset = CGSize(
                    width: offsetBeforeDrag.width + drag.translation.width,
                    height: offsetBeforeDrag.height + drag.translation.height
                )
            }
            .onEnded { _ in offsetBeforeDrag = offset }
    }

    private var pinch: some Gesture {
        MagnifyGesture()
            .onChanged { pinch in
                apply(zoom: zoomBeforePinch * pinch.magnification, from: zoomBeforePinch, offset: offsetBeforeDrag)
            }
            .onEnded { _ in settle() }
    }

    private func close() { store.zoomedImage = nil }

    /// Back to the fitted size and centred at 1; otherwise the visible centre stays put.
    private func setZoom(_ value: CGFloat) {
        if value == 1 {
            zoom = 1
            offset = .zero
        } else {
            apply(zoom: value, from: zoom, offset: offset)
        }
        settle()
    }

    /// Scales around `anchor`, a point measured from the area's centre that stays put on screen.
    private func apply(zoom value: CGFloat, from previous: CGFloat, offset start: CGSize, anchor: CGSize = .zero) {
        (zoom, offset) = Self.zoomed(to: value, from: previous, offset: start, anchor: anchor)
    }

    /// `value` within `zooms`, and the offset that keeps `anchor` in place; centred once no larger than fitted.
    static func zoomed(to value: CGFloat, from previous: CGFloat, offset start: CGSize, anchor: CGSize) -> (zoom: CGFloat, offset: CGSize) {
        let next = min(max(value, zooms.lowerBound), zooms.upperBound)
        guard next > 1 else { return (next, .zero) }
        let ratio = next / previous
        return (next, CGSize(
            width: anchor.width - ratio * (anchor.width - start.width),
            height: anchor.height - ratio * (anchor.height - start.height)
        ))
    }

    /// The wheel zooms around the pointer over the image's area, and scrolls nothing under the viewer.
    private func zoom(by event: NSEvent) -> NSEvent? {
        guard let window, event.window === window, let content = window.contentView else { return event }
        let point = CGPoint(x: event.locationInWindow.x, y: content.bounds.height - event.locationInWindow.y)
        guard frame.contains(point) else { return event }
        guard area.contains(point), event.scrollingDeltaY != 0 else { return nil }
        let speed: CGFloat = event.hasPreciseScrollingDeltas ? 0.01 : 0.1
        let anchor = CGSize(width: point.x - area.midX, height: point.y - area.midY)
        apply(zoom: zoom * exp(event.scrollingDeltaY * speed), from: zoom, offset: offset, anchor: anchor)
        settle()
        return nil
    }

    private func settle() {
        zoomBeforePinch = zoom
        offsetBeforeDrag = offset
    }

    private func copy() {
        NSPasteboard.copy(picture)
    }

    /// Saved as PNG: the viewer holds the decoded image, not the bytes Teams served.
    private func save() {
        guard let path = FilePanels.saveLocation(for: Self.fileName(for: alt)) else { return }
        do {
            guard let png = picture.pngData else { throw CocoaError(.fileWriteUnknown) }
            try png.write(to: URL(fileURLWithPath: path), options: .atomic)
        } catch {
            NSAlert(error: error).runModal()
        }
    }

    /// Named after the alt text Teams gives a pasted file, less its extension and the path separators a name cannot hold.
    static func fileName(for alt: String) -> String {
        let caption = (alt as NSString).deletingPathExtension
            .replacingOccurrences(of: "/", with: "-")
            .replacingOccurrences(of: ":", with: "-")
        return (meaningful(alt) && !caption.isEmpty ? caption : "Image") + ".png"
    }

    /// Native size, shrunk to the area below the bar less its side margins if needed (never enlarged past 2×).
    static func fittedSize(of native: CGSize, in area: CGSize) -> CGSize {
        let bounds = CGSize(width: max(area.width - 2 * 64, 1), height: max(area.height - 2 * 24, 1))
        guard native.width > 0, native.height > 0 else { return bounds }
        let scale = min(bounds.width / native.width, bounds.height / native.height, 2)
        return CGSize(width: native.width * scale, height: native.height * scale)
    }
}

/// "IMPORTANT!" or "URGENT!" above a message, as Teams marks it.
struct ImportanceTag: View {
    let importance: Importance
    var body: some View {
        Label(Self.label(importance), systemImage: importance == .urgent ? "bell.badge.fill" : "exclamationmark")
            .font(.figtree(11, .semibold))
            .foregroundStyle(Theme.errorText)
    }
    static func label(_ importance: Importance) -> String {
        switch importance {
        case .urgent: L("ui.composer.urgent_tag")
        case .high: L("ui.composer.important_tag")
        case .standard: ""
        }
    }
}
